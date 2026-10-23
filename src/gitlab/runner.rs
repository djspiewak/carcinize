use super::{
    api::{Api, Poll, Registration, USER_AGENT},
    context::Context,
    preparation,
    reporting::{self, Outcome, Timing},
    trace,
};
use crate::{
    Config, RunSummary,
    config::Provider,
    error::{Failure, Result},
    http::Http,
    lifecycle::{self, Supervisor},
};
use std::{future::Future, pin::Pin, time::Duration};
use uuid::Uuid;

pub(crate) async fn run(
    config: Config,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<RunSummary> {
    let Provider::Gitlab(host) = &config.provider else {
        return Err(Failure::Config("GitLab configuration required"));
    };
    let api = Api::new(Http::with_user_agent(USER_AGENT)?, host)?;
    run_with(config, api, shutdown, Timing::default()).await
}

pub(super) async fn run_with(
    config: Config,
    api: Api,
    shutdown: impl Future<Output = std::io::Result<()>>,
    timing: Timing,
) -> Result<RunSummary> {
    tokio::pin!(shutdown);
    let project = tokio::select! {
        result = api.project(&config) => result?,
        signal = &mut shutdown => { signal?; return Ok(RunSummary::default()); }
    };
    let (registered, signal) =
        lifecycle::acquire(api.register(&config, project), shutdown.as_mut()).await;
    let registered = registered?;
    if let Some(signal) = signal {
        return lifecycle::combine(
            signal
                .map(|()| RunSummary::default())
                .map_err(Failure::from),
            remove(&api, &config, registered.id, timing).await,
        );
    }
    let result = async {
        if registered.id == 0 || registered.token.is_empty() {
            return Err(Failure::Protocol("invalid GitLab runner registration"));
        }
        let suffix: String = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(12)
            .collect();
        let system_id = format!("r_{suffix}");
        api.verify(&registered, &system_id).await?;
        eprintln!(
            "carcinize: GitLab runner {} registered (id {}); listening for jobs",
            config.name, registered.id
        );
        listen(
            &api,
            &config,
            &registered,
            &system_id,
            project,
            shutdown.as_mut(),
            timing,
        )
        .await
    }
    .await;
    let cleanup = remove(&api, &config, registered.id, timing).await;
    lifecycle::combine(result, cleanup)
}

async fn remove(api: &Api, config: &Config, id: u64, timing: Timing) -> Result<()> {
    if id == 0 {
        return Err(Failure::Protocol(
            "registration response omitted GitLab runner identity; cleanup could not be confirmed",
        ));
    }
    // Only deletion of our known identity is idempotent. Creation must not retry.
    // Four 30-second requests plus 2/4/6-second backoffs bound cleanup to 132 seconds.
    lifecycle::retry(
        || api.remove(config, id),
        Failure::transient,
        3,
        timing.retry,
    )
    .await
}

async fn listen<F: Future<Output = std::io::Result<()>>>(
    api: &Api,
    config: &Config,
    registered: &Registration,
    system_id: &str,
    project: u64,
    mut shutdown: Pin<&mut F>,
    timing: Timing,
) -> Result<RunSummary> {
    let mut summary = RunSummary::default();
    let mut cursor = None;
    let mut failures = 0;
    loop {
        let (polled, signal) = lifecycle::acquire(
            api.poll(&registered.token, system_id, &mut cursor),
            shutdown.as_mut(),
        )
        .await;
        let polled = match polled {
            Ok(p) => {
                failures = 0;
                p
            }
            Err(e) if e.transient() && failures < 3 && signal.is_none() => {
                failures += 1;
                tokio::select! {
                    result = &mut shutdown => { result?; return Ok(summary); }
                    _ = tokio::time::sleep(timing.retry * failures) => {}
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        match polled {
            Poll::Idle => {
                if let Some(result) = signal {
                    result?;
                    return Ok(summary);
                }
                tokio::select! {
                    result = &mut shutdown => { result?; return Ok(summary); }
                    _ = tokio::time::sleep(timing.idle) => {}
                }
            }
            Poll::Job(payload) => {
                let context = Context::identity(&payload, project)?;
                if context.project_id != project {
                    // Never use the administrative token to cancel a job in another project.
                    let error =
                        Failure::Protocol("GitLab assigned a job outside the configured project");
                    api.update(&context, "failed", &[], Some("runner_system_failure"))
                        .await?;
                    return Err(error);
                }
                eprintln!("carcinize: inspecting GitLab job {}", context.id);
                let (outcome, stop) = inspect_job(
                    api,
                    config,
                    &context,
                    (payload, registered.token.clone()),
                    shutdown.as_mut(),
                    signal,
                    timing,
                )
                .await?;
                summary.record(outcome);
                eprintln!(
                    "carcinize: GitLab job {} completed: {outcome:?}",
                    context.id
                );
                if stop {
                    return Ok(summary);
                }
            }
        }
    }
}

async fn inspect_job<F: Future<Output = std::io::Result<()>>>(
    api: &Api,
    config: &Config,
    context: &Context,
    input: (serde_json::Value, String),
    mut shutdown: Pin<&mut F>,
    signal: Option<std::io::Result<()>>,
    timing: Timing,
) -> Result<(Outcome, bool)> {
    let (payload, runner_token) = input;
    let timeout = Duration::from_secs(
        payload
            .pointer("/runner_info/timeout")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(60)
            .min(24 * 60 * 60),
    );
    let deadline = tokio::time::Instant::now() + timeout;
    let admin = config.token.clone();
    let timestamp = chrono::Utc::now().timestamp();
    let mut supervisor = Supervisor::new(signal, Some(deadline));
    let prepared = crate::preparation::owned(
        move |canceled| preparation::prepare(payload, &admin, &runner_token, &canceled, timestamp),
        supervisor.interrupt(
            reporting::heartbeat(api, context, timing),
            shutdown.as_mut(),
        ),
    )
    .await;
    let preparation_cancelled = matches!(prepared, Err(Failure::Cancelled));
    let mut failed = prepared.is_err();
    let trace = match prepared {
        Ok(trace) => trace,
        Err(ref error) => trace::diagnostic(error, &context.redactor),
    };
    let mut offset = 0;
    let result = if preparation_cancelled {
        Err(Failure::Cancelled)
    } else {
        supervisor
            .run(
                reporting::upload(api, context, &trace, &mut offset, timing),
                reporting::heartbeat(api, context, timing),
                shutdown.as_mut(),
            )
            .await
    };
    let cancelled = matches!(result, Err(Failure::Cancelled));
    if let Err(ref error) = result {
        if !cancelled {
            failed = true;
        }
        eprintln!(
            "carcinize: GitLab job {}: {}",
            context.id,
            context.redactor.redact(&error.to_string())
        );
    }
    let mut cancel = if supervisor.signal.is_some() {
        api.cancel(config, context).await
    } else {
        Ok(())
    };
    // Retain every byte that might have reached the server. A lost PATCH response
    // is reconciled through 412/416, including failed and canceled completion.
    let requested = if cancelled {
        Outcome::Cancelled
    } else if failed {
        Outcome::Failed
    } else {
        Outcome::Success
    };
    let finishing = reporting::finish(api, config, context, &trace, requested, timing);
    tokio::pin!(finishing);
    let outcome = if supervisor.signal.is_some() {
        finishing.await
    } else {
        tokio::select! {
            result = &mut finishing => result,
            result = &mut shutdown => {
                supervisor.signal = Some(result);
                // Keep the in-flight update owned; cancellation changes the remote
                // state while the same completion protocol drains to its deadline.
                cancel = api.cancel(config, context).await;
                finishing.await
            }
        }
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            // A terminal timeout or exhausted reconciliation must not abandon a
            // known running assignment just because the runner is being removed.
            let cleanup = async {
                api.cancel(config, context).await?;
                reporting::finish(api, config, context, &trace, Outcome::Cancelled, timing).await
            };
            match tokio::time::timeout(timing.finish, cleanup).await {
                Ok(Ok(_)) => {}
                Ok(Err(cleanup)) => {
                    eprintln!("carcinize: GitLab job cleanup unresolved: {cleanup}")
                }
                Err(_) => eprintln!("carcinize: GitLab job cleanup unresolved: deadline exceeded"),
            }
            return Err(error);
        }
    };
    supervisor.finish(lifecycle::combine(Ok(outcome), cancel))
}
