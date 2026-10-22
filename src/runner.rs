use crate::{
    auth::{Identity, RunnerAuth, register_runner, remove_runner},
    broker::{Broker, Message},
    config::Config,
    error::{Error, Failure, Result},
    http::{Http, service_url},
    job_context::{Conclusion, JobContext},
    model::Job,
    reporting::Reporter,
};
use serde::Serialize;
use std::{future::Future, pin::Pin, time::Duration};
use url::Url;

/// Counts from this invocation; successful jobs completed inspection and required reporting.
#[derive(Debug, Default, Serialize)]
pub struct RunSummary {
    /// Jobs successfully inspected.
    pub jobs_inspected: u64,
    /// Jobs that failed decoding, inspection, or reporting.
    pub jobs_failed: u64,
    /// Jobs cancelled by GitHub or local shutdown.
    pub jobs_cancelled: u64,
}

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
        Http::new()?,
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
    let registered = {
        let registration = register_runner(&http, &api, &config, &identity);
        tokio::pin!(registration);
        tokio::select! {
        result = &mut registration => result?,
        signal = &mut shutdown => {
            // Finish a registration already in flight so its server identity can be removed.
            let registered = registration.await?;
            remove_runner(&http, &api, &config, registered.id).await?;
            signal?;
            return Ok(RunSummary::default());
        }
        }
    };
    let id = registered.id;
    let result = async {
        let auth = RunnerAuth::new(identity, &registered.authorization)?;
        let mut broker = Broker::connect(
            http.clone(),
            auth,
            service_url(&registered.authorization.server_url)?,
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
        combine(result, cleanup)
    }
    .await;
    let cleanup = remove_runner(&http, &api, &config, id).await;
    combine(result, cleanup)
}

fn combine<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Err(cleanup)) => {
            eprintln!("carcinize: cleanup also failed: {cleanup}");
            Err(error)
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
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
        let context = JobContext::decode(&payload)?;
        eprintln!("carcinize: inspecting job {}", context.id);
        let job = Job::decode(payload);
        let (conclusion, stop) = match job {
            Ok(job) => {
                inspect_job(
                    http,
                    config,
                    broker,
                    &context,
                    &job,
                    shutdown.as_mut(),
                    renewal_period,
                )
                .await?
            }
            Err(error) => {
                context
                    .complete(http, Conclusion::Failed, &[], Some(&error.to_string()))
                    .await?;
                (Conclusion::Failed, false)
            }
        };
        match conclusion {
            Conclusion::Success => summary.jobs_inspected += 1,
            Conclusion::Failed => summary.jobs_failed += 1,
            Conclusion::Cancelled => summary.jobs_cancelled += 1,
        }
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
    job: &Job,
    mut shutdown: Pin<&mut F>,
    renewal_period: Duration,
) -> Result<(Conclusion, bool)> {
    let setup = context
        .redactor(&config.token)
        .and_then(|redactor| Reporter::new(http.clone(), context, redactor, job));
    let mut reporter = match setup {
        Ok(reporter) => reporter,
        Err(error) => {
            context
                .complete(http, Conclusion::Failed, &[], Some(&error.to_string()))
                .await?;
            return Ok((Conclusion::Failed, false));
        }
    };
    let mut stop = false;
    let inspection = match context.renew(http).await {
        Err(error) => Err(error),
        Ok(locked_until) => {
            let renewal = context.maintain_lease(http, locked_until, renewal_period);
            let inspect = reporter.inspect(job);
            tokio::pin!(renewal, inspect);
            loop {
                tokio::select! {
                    result = &mut inspect => break result,
                    result = &mut renewal => break result,
                    signal = &mut shutdown => {
                        stop = true;
                        break match signal { Ok(()) => Err(Failure::Cancelled), Err(e) => Err(e.into()) };
                    },
                    message = broker.poll(true) => match message {
                        Ok(Some(Message::Cancel(id))) if id == context.id => break Err(Failure::Cancelled),
                        Ok(Some(Message::Job(_))) => break Err(Failure::Protocol("broker assigned work while runner is busy")),
                        Ok(_) => {},
                        Err(error) => break Err(error),
                    },
                }
            }
        }
    };
    Ok((reporter.finish(inspection).await?, stop))
}
