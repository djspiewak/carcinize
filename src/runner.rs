use crate::{Config, Error, config::Provider, github, gitlab};
use serde::Serialize;
use std::future::Future;

/// Counts confirmed outcomes; successful jobs completed inspection and required reporting.
#[derive(Debug, Default, Serialize)]
pub struct RunSummary {
    /// Jobs successfully inspected.
    pub jobs_inspected: u64,
    /// Jobs that failed decoding, inspection, or reporting.
    pub jobs_failed: u64,
    /// Jobs cancelled remotely or by local shutdown.
    pub jobs_cancelled: u64,
}

/// Register a transient runner, inspect jobs sequentially, and unregister on shutdown.
///
/// The shutdown future completes when the caller requests shutdown. Credentials remain in
/// memory. Network, protocol, and cleanup failures return an error; no job commands execute.
pub async fn run(
    config: Config,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<RunSummary, Error> {
    match &config.provider {
        Provider::Github => github::runner::run(config, shutdown).await,
        Provider::Gitlab(_) => Ok(gitlab::runner::run(config, shutdown).await?),
    }
}
