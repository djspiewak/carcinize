use crate::RunSummary;
use crate::{
    config::Config,
    error::{Error, Failure, Result},
    github::auth::{Identity, RunnerAuth, register_runner, remove_runner},
    github::broker::{Broker, Message},
    github::job_context::{Conclusion, JobContext},
    github::preparation,
    github::reporting::Reporter,
    http::{Http, service_url},
    lifecycle::{self, Supervisor},
};
use futures_util::FutureExt;
use std::{future::Future, pin::Pin, time::Duration};
use url::Url;

/// Register on GitHub.com, inspect jobs sequentially, and unregister on shutdown.
///
/// The shutdown future should complete when the caller requests shutdown (for example Ctrl-C).
/// Network, protocol, and cleanup failures return an error. Credentials and keys are kept in memory.
pub async fn run(
    config: Config,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> std::result::Result<RunSummary, Error> {
    let identity = tokio::task::spawn_blocking(Identity::generate)
        .await
        .map_err(Failure::from)??;
    Ok(run_with(
        config,
        Http::with_user_agent(super::USER_AGENT)?,
        service_url("https://api.github.com/")?,
        identity,
        shutdown,
        Duration::from_secs(60),
    )
    .await?)
}

pub(crate) async fn run_with(
    config: Config,
    http: Http,
    api: Url,
    identity: Identity,
    shutdown: impl Future<Output = std::io::Result<()>>,
    renewal_period: Duration,
) -> Result<RunSummary> {
    tokio::pin!(shutdown);
    let (registered, signal) = lifecycle::acquire(
        register_runner(&http, &api, &config, &identity),
        shutdown.as_mut(),
    )
    .await;
    let registered = registered?;
    if let Some(signal) = signal {
        return lifecycle::combine(
            signal
                .map(|()| RunSummary::default())
                .map_err(Failure::from),
            remove(&http, &api, &config, registered.id).await,
        );
    }
    let id = registered.id;
    let result = async {
        let authorization = registered.authorization?;
        let auth = RunnerAuth::new(identity, &authorization)?;
        let mut broker = Broker::connect(
            http.clone(),
            auth,
            service_url(&authorization.server_url)?,
            id,
            &config.name,
        )
        .await?;
        eprintln!(
            "carcinize: runner {} registered (id {id}); listening for jobs",
            config.name
        );
        let result = listen(
            &http,
            &config,
            &mut broker,
            shutdown.as_mut(),
            renewal_period,
        )
        .await;
        let cleanup = broker.close().await;
        lifecycle::combine(result, cleanup)
    }
    .await;
    let cleanup = remove(&http, &api, &config, id).await;
    lifecycle::combine(result, cleanup)
}

async fn remove(http: &Http, api: &Url, config: &Config, id: u64) -> Result<()> {
    lifecycle::retry(
        || remove_runner(http, api, config, id),
        Failure::transient,
        3,
        Duration::from_secs(2),
    )
    .await
}

async fn listen<F: Future<Output = std::io::Result<()>>>(
    http: &Http,
    config: &Config,
    broker: &mut Broker,
    mut shutdown: Pin<&mut F>,
    renewal_period: Duration,
) -> Result<RunSummary> {
    let mut summary = RunSummary::default();
    let mut failures = 0u32;
    loop {
        let message = tokio::select! {
            signal = &mut shutdown => { signal?; break; },
            message = broker.poll(false) => message,
        };
        let message = match message {
            Ok(message) => {
                failures = 0;
                message
            }
            Err(error) if error.transient() && failures < 3 => {
                failures += 1;
                eprintln!("carcinize: {error}; retrying poll");
                tokio::select! {
                    signal = &mut shutdown => { signal?; break; },
                    _ = tokio::time::sleep(Duration::from_secs(1 << failures)) => {},
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        let Some(Message::Job(assignment)) = message else {
            continue;
        };
        let payload = match broker.acquire(&assignment).await {
            Ok(payload) => payload,
            Err(error) if error.is_status(404) || error.is_status(409) || error.is_status(422) => {
                eprintln!("carcinize: {error}; assignment is no longer available");
                continue;
            }
            Err(error) => return Err(error),
        };
        let context = JobContext::identity(&payload)?;
        eprintln!("carcinize: inspecting job {}", context.id);
        let (conclusion, stop) = inspect_job(
            http,
            config,
            broker,
            &context,
            payload,
            shutdown.as_mut(),
            renewal_period,
        )
        .await?;
        summary.record(conclusion);
        eprintln!("carcinize: job {} completed: {conclusion:?}", context.id);
        if stop {
            break;
        }
    }
    Ok(summary)
}

async fn inspect_job<F: Future<Output = std::io::Result<()>>>(
    http: &Http,
    config: &Config,
    broker: &mut Broker,
    context: &JobContext,
    payload: serde_json::Value,
    mut shutdown: Pin<&mut F>,
    renewal_period: Duration,
) -> Result<(Conclusion, bool)> {
    let mut supervisor = Supervisor::new(
        None,
        Some(tokio::time::Instant::now() + Duration::from_secs(24 * 60 * 60)),
    );
    // Establish the lease before expensive work, then retain the same monitor
    // across every phase. A failed monitor is fused so cleanup cannot repoll it.
    let locked_until = match supervisor
        .run(
            context.renew(http),
            std::future::pending(),
            shutdown.as_mut(),
        )
        .await
    {
        Ok(locked_until) => locked_until,
        Err(error) => {
            let conclusion = if matches!(error, Failure::Cancelled) {
                Conclusion::Cancelled
            } else {
                Conclusion::Failed
            };
            let (result, _) = supervisor
                .drain(
                    context.complete(http, conclusion, &[], Some(&error.to_string())),
                    std::future::pending(),
                    shutdown.as_mut(),
                )
                .await;
            return supervisor.finish(result.map(|()| conclusion));
        }
    };
    let liveness = monitor(http, broker, context, locked_until, renewal_period).fuse();
    tokio::pin!(liveness);
    let admin = config.token.clone();
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let prepared = crate::preparation::owned(
        move |canceled| preparation::prepare(payload, &admin, &canceled, &timestamp),
        supervisor.interrupt(liveness.as_mut(), shutdown.as_mut()),
    )
    .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let conclusion = if matches!(error, Failure::Cancelled) {
                Conclusion::Cancelled
            } else {
                Conclusion::Failed
            };
            // Preparation diagnostics are static; never reflect an unmasked payload.
            let (result, _) = supervisor
                .drain(
                    context.complete(http, conclusion, &[], Some(&error.to_string())),
                    liveness.as_mut(),
                    shutdown.as_mut(),
                )
                .await;
            return supervisor.finish(result.map(|()| conclusion));
        }
    };
    let mut reporter = match Reporter::new(
        http.clone(),
        &prepared.context,
        prepared.redactor,
        &prepared.steps,
    ) {
        Ok(reporter) => reporter,
        Err(error) => {
            let (result, _) = supervisor
                .drain(
                    context.complete(http, Conclusion::Failed, &[], Some(&error.to_string())),
                    liveness.as_mut(),
                    shutdown.as_mut(),
                )
                .await;
            return supervisor.finish(result.map(|()| Conclusion::Failed));
        }
    };
    let mut inspection = supervisor
        .run(
            reporter.inspect(&prepared.steps),
            liveness.as_mut(),
            shutdown.as_mut(),
        )
        .await;
    reporter.settle(&inspection);
    let (published, interruption) = supervisor
        .drain(reporter.publish(), liveness.as_mut(), shutdown.as_mut())
        .await;
    if let Some(error) = interruption
        && (matches!(error, Failure::Cancelled) || inspection.is_ok())
    {
        inspection = Err(error);
    }
    inspection = lifecycle::combine(inspection, published);
    // Observe any already-ready interruption before committing the terminal
    // outcome. This checkpoint performs no I/O and cannot abandon a write.
    if let Err(error) = supervisor
        .run(
            std::future::ready(Ok(())),
            liveness.as_mut(),
            shutdown.as_mut(),
        )
        .await
        && (matches!(error, Failure::Cancelled) || inspection.is_ok())
    {
        inspection = Err(error);
    }
    // Publication is drained before choosing the terminal request, so cancellation
    // or lease loss during the final log upload cannot submit success.
    let (completed, _) = supervisor
        .drain(
            reporter.complete(inspection),
            liveness.as_mut(),
            shutdown.as_mut(),
        )
        .await;
    supervisor.finish(completed)
}

async fn monitor(
    http: &Http,
    broker: &mut Broker,
    context: &JobContext,
    locked_until: chrono::DateTime<chrono::Utc>,
    period: Duration,
) -> Result<()> {
    let renewal = context.maintain_lease(http, locked_until, period);
    tokio::pin!(renewal);
    loop {
        tokio::select! {
            result = &mut renewal => return result,
            message = broker.poll(true) => match message? {
                Some(Message::Cancel(id)) if id == context.id => return Err(Failure::Cancelled),
                Some(Message::Job(_)) => return Err(Failure::Protocol("broker assigned work while runner is busy")),
                _ => {},
            }
        }
    }
}
