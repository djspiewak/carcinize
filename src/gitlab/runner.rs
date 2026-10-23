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
    let registered = {
        let registration = api.register(&config, project);
        tokio::pin!(registration);
        tokio::select! {
            result = &mut registration => result?,
            signal = &mut shutdown => {
                // Creation is not cancellation-safe: recover the ID before removing it.
                let registered = registration.await?;
                remove(&api, &config, registered.id, timing).await?;
                signal?;
                return Ok(RunSummary::default());
            }
        }
    };
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
    match (result, cleanup) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Err(error), Err(cleanup)) => {
            eprintln!("carcinize: cleanup also failed: {cleanup}");
            Err(error)
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

async fn remove(api: &Api, config: &Config, id: u64, timing: Timing) -> Result<()> {
    if id == 0 {
        return Err(Failure::Protocol(
            "registration response omitted GitLab runner identity; cleanup could not be confirmed",
        ));
    }
    // Only deletion of our known identity is idempotent. Creation must not retry.
    // Four 30-second requests plus 2/4/6-second backoffs bound cleanup to 132 seconds.
    let mut retries = 0;
    loop {
        match api.remove(config, id).await {
            Err(error) if error.transient() && retries < 3 => {
                retries += 1;
                tokio::time::sleep(timing.retry * retries).await;
            }
            result => return result,
        }
    }
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
        let mut signal = None;
        let polled = {
            let poll = api.poll(&registered.token, system_id, &mut cursor);
            tokio::pin!(poll);
            tokio::select! {
                result = &mut poll => result,
                result = &mut shutdown => {
                    signal = Some(result);
                    // The server may have assigned a job even if our response is still pending.
                    poll.await
                }
            }
        };
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
                match outcome {
                    Outcome::Success => summary.jobs_inspected += 1,
                    Outcome::Failed => summary.jobs_failed += 1,
                    Outcome::Cancelled => summary.jobs_cancelled += 1,
                }
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
    mut signal: Option<std::io::Result<()>>,
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
    let prepared = preparation::owned(
        move |canceled| preparation::prepare(payload, &admin, &runner_token, &canceled),
        async {
            if signal.is_some() { return Failure::Cancelled; }
            tokio::select! {
                result = &mut shutdown => { signal = Some(result); Failure::Cancelled }
                result = reporting::heartbeat(api, context, timing) => result.err().unwrap_or(Failure::Cancelled),
                _ = tokio::time::sleep_until(deadline) => Failure::Protocol("GitLab inspection timed out"),
            }
        },
    ).await;
    let preparation_cancelled = matches!(prepared, Err(Failure::Cancelled));
    let mut failed = prepared.is_err();
    let trace = match prepared {
        Ok(trace) => trace,
        Err(ref error) => trace::diagnostic(error, &context.redactor),
    };
    let mut offset = 0;
    let result = if signal.is_some() || preparation_cancelled {
        Err(Failure::Cancelled)
    } else if tokio::time::Instant::now() >= deadline {
        Err(Failure::Protocol("GitLab inspection timed out"))
    } else {
        tokio::select! {
            result = reporting::inspect(api, context, &trace, &mut offset, timing) => result,
            result = &mut shutdown => { signal = Some(result); Err(Failure::Cancelled) }
            _ = tokio::time::sleep_until(deadline) => Err(Failure::Protocol("GitLab inspection timed out")),
        }
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
    let mut cancel = if signal.is_some() {
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
    let outcome = if signal.is_some() {
        finishing.await
    } else {
        tokio::select! {
            result = &mut finishing => result,
            result = &mut shutdown => {
                signal = Some(result);
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
    cancel?;
    let stop = signal.is_some();
    if let Some(result) = signal {
        result?;
    }
    Ok((outcome, stop))
}
