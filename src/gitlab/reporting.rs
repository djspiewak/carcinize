use super::{
    api::{self, Api, RemoteState},
    context::Context,
    trace::Trace,
};
use crate::{
    Config,
    error::{Failure, Result},
};
use std::time::Duration;

const PATCH_SIZE: usize = 64 * 1024;

pub(super) use crate::lifecycle::Outcome;

#[derive(Clone, Copy)]
pub(super) struct Timing {
    pub idle: Duration,
    pub retry: Duration,
    pub heartbeat: Duration,
    pub finish: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(3),
            retry: Duration::from_secs(2),
            heartbeat: Duration::from_secs(3),
            finish: Duration::from_secs(120),
        }
    }
}

fn stopped(state: Option<RemoteState>) -> Result<()> {
    match state {
        Some(RemoteState::Canceled | RemoteState::Canceling) => Err(Failure::Cancelled),
        Some(RemoteState::Failed) => Err(Failure::Protocol("GitLab stopped the job as failed")),
        Some(RemoteState::Success) => Err(Failure::Protocol(
            "GitLab completed the job before inspection finished",
        )),
        Some(RemoteState::Running) | None => Ok(()),
    }
}

pub(super) async fn heartbeat(api: &Api, context: &Context, timing: Timing) -> Result<()> {
    let mut period = timing.heartbeat;
    let mut retries = 0;
    loop {
        tokio::time::sleep(period).await;
        let error = match api.update(context, "running", &[], None).await {
            Ok(response) => {
                stopped(api::state(&response)?)?;
                if response.status().as_u16() == 200 {
                    retries = 0;
                    period = api::interval(&response, timing.heartbeat);
                    continue;
                }
                api::http_error(&response, "heartbeat GitLab job")
            }
            Err(error) => error,
        };
        if error.transient() && retries < 3 {
            retries += 1;
            period = timing.retry * retries;
        } else {
            return Err(error);
        }
    }
}

pub(super) async fn upload(
    api: &Api,
    context: &Context,
    trace: &Trace,
    progress: &mut usize,
    timing: Timing,
) -> Result<()> {
    upload_inner(api, context, trace, progress, timing, false).await
}

async fn upload_inner(
    api: &Api,
    context: &Context,
    trace: &Trace,
    progress: &mut usize,
    timing: Timing,
    completing: bool,
) -> Result<()> {
    let mut offset = *progress;
    let mut retries = 0;
    while offset < trace.bytes.len() {
        let boundary = trace
            .sections
            .iter()
            .copied()
            .find(|end| *end > offset)
            .unwrap_or(trace.bytes.len());
        let end = boundary.min(offset + PATCH_SIZE);
        let bytes = trace
            .bytes
            .get(offset..end)
            .ok_or(Failure::Protocol("invalid local trace offset"))?;
        let response = match api.patch(context, bytes, offset).await {
            Ok(response) => response,
            Err(e) if e.transient() && retries < 3 => {
                retries += 1;
                tokio::time::sleep(timing.retry * retries).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        let remote = api::state(&response)?;
        // Graceful cancellation still permits the final trace to be reconciled.
        if !completing || remote != Some(RemoteState::Canceling) {
            stopped(remote)?;
        }
        match response.status().as_u16() {
            202 => {
                offset = end;
                retries = 0;
            }
            416 if retries < 3 => {
                offset = range_offset(
                    response
                        .headers()
                        .get("Range")
                        .and_then(|h| h.to_str().ok()),
                    trace.bytes.len(),
                )?;
                retries += 1;
            }
            429 | 500..=599 if retries < 3 => {
                retries += 1;
                tokio::time::sleep(timing.retry * retries).await;
            }
            _ => return Err(api::http_error(&response, "append GitLab trace")),
        }
        *progress = offset;
    }
    Ok(())
}

fn range_offset(range: Option<&str>, size: usize) -> Result<usize> {
    // GitLab's Range is 0-length, despite the inclusive request Content-Range convention.
    range
        .and_then(|s| s.strip_prefix("0-"))
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|offset| *offset <= size)
        .ok_or(Failure::Protocol("invalid GitLab trace range"))
}

pub(super) async fn finish(
    api: &Api,
    config: &Config,
    context: &Context,
    trace: &Trace,
    outcome: Outcome,
    timing: Timing,
) -> Result<Outcome> {
    tokio::time::timeout(
        timing.finish,
        finish_inner(api, config, context, trace, outcome, timing),
    )
    .await
    .map_err(|_| Failure::Protocol("GitLab job completion timed out"))?
}

async fn finish_inner(
    api: &Api,
    config: &Config,
    context: &Context,
    trace: &Trace,
    outcome: Outcome,
    timing: Timing,
) -> Result<Outcome> {
    let (mut target, mut reason) = match outcome {
        Outcome::Success => ("success", None),
        Outcome::Failed => ("failed", Some("runner_system_failure")),
        Outcome::Cancelled => ("failed", Some("job_canceled")),
    };
    let mut retries = 0;
    loop {
        let response = match api.update(context, target, &trace.bytes, reason).await {
            Ok(r) => r,
            Err(e) if e.transient() && retries < 3 => {
                retries += 1;
                tokio::time::sleep(timing.retry * retries).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        let remote = if matches!(response.status().as_u16(), 403 | 404) {
            // Job tokens can be revoked immediately on cancellation or successful completion,
            // including when the terminal acknowledgment was lost. Confirm through the project.
            Some(api.job_state(config, context).await?)
        } else {
            api::state(&response)?
        };
        if remote == Some(RemoteState::Canceled) {
            return Ok(Outcome::Cancelled);
        }
        if remote == Some(RemoteState::Failed) {
            return Ok(Outcome::Failed);
        }
        if remote == Some(RemoteState::Success)
            && target == "success"
            && matches!(response.status().as_u16(), 403 | 404)
        {
            return Ok(Outcome::Success);
        }
        if remote == Some(RemoteState::Canceling) {
            target = "failed";
            reason = Some("job_canceled");
        }
        match response.status().as_u16() {
            200 if remote == Some(RemoteState::Success) && target == "success" => {
                return Ok(Outcome::Success);
            }
            202 | 200 if remote != Some(RemoteState::Success) => {
                tokio::time::sleep(api::interval(&response, timing.retry)).await;
            }
            412 if retries < 3 => {
                retries += 1;
                upload_inner(api, context, trace, &mut 0, timing, true).await?;
            }
            429 | 500..=599 if retries < 3 => {
                retries += 1;
                tokio::time::sleep(timing.retry * retries).await;
            }
            _ => return Err(api::http_error(&response, "complete GitLab job")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gitlab_range_is_a_byte_count_and_must_fit_the_trace() {
        assert_eq!(range_offset(Some("0-3"), 3).expect("valid"), 3);
        for value in [
            None,
            Some("0-4"),
            Some("1-2"),
            Some("0--1"),
            Some("0-18446744073709551616"),
        ] {
            assert!(range_offset(value, 3).is_err());
        }
    }
}
