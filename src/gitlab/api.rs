use super::context::Context;
use crate::{
    Config,
    error::{Failure, Result},
    http::{Http, POLL_TIMEOUT, endpoint},
};
use reqwest::{Response, header::HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use url::Url;

pub(super) const VERSION: &str = "17.7.1";
pub(super) const USER_AGENT: &str = "carcinize/0.1.0 (GitLab Runner protocol 17.7.1)";

pub(super) struct Api {
    pub http: Http,
    pub base: Url,
}

#[derive(Deserialize)]
pub(super) struct Registration {
    pub id: u64,
    #[serde(default, deserialize_with = "registration_token")]
    pub token: String,
}

// A malformed token must not discard the ID of a runner we already created.
fn registration_token<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    Ok(Value::deserialize(deserializer)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

pub(super) enum Poll {
    Idle,
    Job(Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteState {
    Running,
    Canceling,
    Canceled,
    Failed,
    Success,
}

pub(super) fn state(response: &Response) -> Result<Option<RemoteState>> {
    response
        .headers()
        .get("Job-Status")
        .map(|v| match v.to_str().ok() {
            Some("running") => Ok(RemoteState::Running),
            Some("canceling") => Ok(RemoteState::Canceling),
            Some("canceled") => Ok(RemoteState::Canceled),
            Some("failed") => Ok(RemoteState::Failed),
            Some("success") => Ok(RemoteState::Success),
            _ => Err(Failure::Protocol("unknown GitLab job status")),
        })
        .transpose()
}

pub(super) fn interval(response: &Response, fallback: Duration) -> Duration {
    response
        .headers()
        .get("X-GitLab-Trace-Update-Interval")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(|s| Duration::from_secs(s.min(60)))
        .unwrap_or(fallback)
}

pub(super) fn http_error(response: &Response, operation: &'static str) -> Failure {
    Failure::Http {
        operation,
        status: response.status().as_u16(),
    }
}

fn credential(value: &str) -> Result<HeaderValue> {
    let mut header =
        HeaderValue::from_str(value).map_err(|_| Failure::Protocol("invalid GitLab credential"))?;
    header.set_sensitive(true);
    Ok(header)
}

fn info() -> Value {
    json!({"name":"carcinize", "version":VERSION,
    "revision":"6669abcf", "platform": if std::env::consts::OS == "macos" { "darwin" } else { std::env::consts::OS },
    "architecture": match std::env::consts::ARCH { "aarch64" => "arm64", "x86_64" => "amd64", other => other },
    "executor":"custom", "features": {
        "variables":true, "image":true, "services":true, "artifacts":true, "cache":true,
        "refspecs":true, "masking":true, "raw_variables":true, "multi_build_steps":true,
        "trace_checksum":true, "trace_size":true, "cancelable":true, "cancel_gracefully":true,
        "native_steps_integration":true
    }})
}

impl Api {
    pub async fn job_state(&self, config: &Config, context: &Context) -> Result<RemoteState> {
        #[derive(Deserialize)]
        struct JobState {
            status: String,
        }
        let job: JobState = self
            .http
            .json(
                self.http
                    .client
                    .get(endpoint(
                        &self.base,
                        &format!("projects/{}/jobs/{}", context.project_id, context.id),
                    )?)
                    .header("PRIVATE-TOKEN", credential(&config.token)?),
                "confirm GitLab job outcome",
            )
            .await?;
        match job.status.as_str() {
            "running" => Ok(RemoteState::Running),
            "canceling" => Ok(RemoteState::Canceling),
            "canceled" => Ok(RemoteState::Canceled),
            "failed" => Ok(RemoteState::Failed),
            "success" => Ok(RemoteState::Success),
            _ => Err(Failure::Protocol("unknown GitLab job outcome")),
        }
    }

    pub fn new(http: Http, host: &Url) -> Result<Self> {
        Ok(Self {
            http,
            base: endpoint(host, "api/v4/")?,
        })
    }

    pub async fn project(&self, config: &Config) -> Result<u64> {
        let mut url = endpoint(&self.base, "projects/")?;
        url.path_segments_mut()
            .map_err(|_| Failure::Protocol("invalid GitLab API base"))?
            .pop_if_empty()
            .push(&config.repository);
        #[derive(Deserialize)]
        struct Project {
            id: u64,
        }
        let p: Project = self
            .http
            .json(
                self.http
                    .client
                    .get(url)
                    .header("PRIVATE-TOKEN", credential(&config.token)?),
                "resolve GitLab project",
            )
            .await?;
        if p.id == 0 {
            return Err(Failure::Protocol("invalid GitLab project identity"));
        }
        Ok(p.id)
    }

    pub async fn register(&self, config: &Config, project: u64) -> Result<Registration> {
        // No retries: the create operation has no idempotency key.
        self.http
            .json(
                self.http
                    .client
                    .post(endpoint(&self.base, "user/runners")?)
                    .header("PRIVATE-TOKEN", credential(&config.token)?)
                    .json(&json!({"runner_type":"project_type", "project_id":project,
                "description":config.name, "tag_list":config.labels, "locked":true,
                "run_untagged":false, "paused":false, "access_level":"not_protected",
                "maintenance_note":"Carcinize inspection only: no job commands execute."})),
                "create GitLab runner",
            )
            .await
    }

    pub async fn remove(&self, config: &Config, id: u64) -> Result<()> {
        let response = self
            .http
            .exchange(
                self.http
                    .client
                    .delete(endpoint(&self.base, &format!("runners/{id}"))?)
                    .header("PRIVATE-TOKEN", credential(&config.token)?),
                "delete GitLab runner",
            )
            .await?;
        match response.status().as_u16() {
            204 | 404 => Ok(()),
            _ => Err(http_error(&response, "delete GitLab runner")),
        }
    }

    pub async fn verify(&self, registered: &Registration, system_id: &str) -> Result<()> {
        self.http
            .send(
                self.http
                    .client
                    .post(endpoint(&self.base, "runners/verify")?)
                    .json(&json!({"token":registered.token, "system_id":system_id})),
                "verify GitLab runner",
            )
            .await?;
        Ok(())
    }

    pub async fn poll(
        &self,
        token: &str,
        system_id: &str,
        last_update: &mut Option<String>,
    ) -> Result<Poll> {
        let response = self.http.exchange(self.http.client.post(endpoint(&self.base, "jobs/request")?)
            .timeout(POLL_TIMEOUT)
            .json(&json!({"token":token,"system_id":system_id,"last_update":last_update,"info":info()})), "request GitLab job").await?;
        if let Some(value) = response.headers().get("X-GitLab-Last-Update") {
            *last_update = Some(
                value
                    .to_str()
                    .map_err(|_| Failure::Protocol("invalid GitLab queue cursor"))?
                    .to_owned(),
            );
        }
        match response.status().as_u16() {
            201 => Ok(Poll::Job(
                self.http
                    .decode(response, "decode GitLab assignment")
                    .await?,
            )),
            204 | 409 => Ok(Poll::Idle),
            _ => Err(http_error(&response, "request GitLab job")),
        }
    }

    pub async fn patch(&self, context: &Context, bytes: &[u8], offset: usize) -> Result<Response> {
        let end = offset
            .checked_add(bytes.len())
            .and_then(|n| n.checked_sub(1))
            .ok_or(Failure::Protocol("invalid GitLab trace patch range"))?;
        self.http
            .exchange(
                self.http
                    .client
                    .patch(endpoint(&self.base, &format!("jobs/{}/trace", context.id))?)
                    .header("JOB-TOKEN", credential(&context.token)?)
                    .header("Content-Range", format!("{offset}-{end}"))
                    .header("Content-Type", "text/plain")
                    .body(bytes.to_vec()),
                "append GitLab trace",
            )
            .await
    }

    pub async fn update(
        &self,
        context: &Context,
        state: &str,
        trace: &[u8],
        reason: Option<&str>,
    ) -> Result<Response> {
        self.http.exchange(self.http.client.put(endpoint(&self.base, &format!("jobs/{}",context.id))?)
            .json(&json!({"token":context.token,"info":info(),"state":state,"failure_reason":reason,
                "output":{"checksum":format!("crc32:{:08x}", crc32fast::hash(trace)),"bytesize":trace.len()}})), "update GitLab job").await
    }

    pub async fn cancel(&self, config: &Config, context: &Context) -> Result<()> {
        self.http
            .send(
                self.http
                    .client
                    .post(endpoint(
                        &self.base,
                        &format!("projects/{}/jobs/{}/cancel", context.project_id, context.id),
                    )?)
                    .header("PRIVATE-TOKEN", credential(&config.token)?),
                "cancel assigned GitLab job",
            )
            .await?;
        Ok(())
    }
}
