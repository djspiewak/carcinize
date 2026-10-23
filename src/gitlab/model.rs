use super::{declarations::*, native::NativeStep, numbers};
use crate::error::{Failure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
pub(super) struct Job {
    pub id: u64,
    pub job_info: JobInfo,
    pub git_info: GitInfo,
    pub runner_info: RunnerInfo,
    #[serde(default, deserialize_with = "null_default")]
    pub allow_git_fetch: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub variables: Vec<Variable>,
    #[serde(default, deserialize_with = "null_default")]
    pub steps: Vec<Step>,
    #[serde(default, deserialize_with = "null_default")]
    pub hooks: Vec<Hook>,
    pub image: Option<Image>,
    #[serde(default, deserialize_with = "null_default")]
    pub services: Vec<Image>,
    #[serde(default, deserialize_with = "null_default")]
    pub artifacts: Vec<Artifact>,
    #[serde(default, deserialize_with = "null_default")]
    pub cache: Vec<Cache>,
    #[serde(default, deserialize_with = "null_default")]
    pub dependencies: Vec<Dependency>,
    #[serde(skip_serializing, default, deserialize_with = "null_default")]
    secrets: SecretNames,
    #[serde(skip_serializing, default, deserialize_with = "null_default")]
    run: String,
    #[serde(skip_deserializing)]
    pub native_steps: Vec<NativeStep>,
}

impl Job {
    pub fn decode(value: Value) -> Result<Self> {
        let mut job: Self = serde_json::from_value(value)
            .map_err(|_| Failure::Protocol("malformed or unsupported GitLab job declaration"))?;
        if job.id == 0 || job.job_info.name.is_empty() || job.runner_info.timeout == 0 {
            return Err(Failure::Protocol("invalid GitLab job identity or timeout"));
        }
        if !job.secrets.is_empty() {
            return Err(Failure::Protocol(
                "external secret-provider declarations are not supported",
            ));
        }
        if !job.run.is_empty() {
            if !job.steps.is_empty() {
                return Err(Failure::Protocol(
                    "GitLab job mixes run and script declarations",
                ));
            }
            job.native_steps = serde_json::from_str(&job.run).map_err(|_| {
                Failure::Protocol("malformed or unsupported GitLab run declaration")
            })?;
            numbers::validate(&job.run)?;
            if job.native_steps.is_empty() || !job.native_steps.iter().all(|s| s.valid(0)) {
                return Err(Failure::Protocol("invalid GitLab run declaration"));
            }
        } else if job.steps.is_empty() {
            return Err(Failure::Protocol("GitLab job contains no declarations"));
        }
        Ok(job)
    }
}
