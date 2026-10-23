//! Decode the reporting identity and masking data before interpreting job semantics.
use super::declarations::{Variable, null_default};
use crate::{
    error::{Failure, Result},
    preparation,
    redact::Redactor,
};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

pub(super) struct Context {
    pub id: u64,
    pub project_id: u64,
    pub token: String,
    pub redactor: Redactor,
}

#[derive(Deserialize)]
struct Envelope {
    id: u64,
    token: String,
    job_info: Project,
    #[serde(default, deserialize_with = "null_default")]
    variables: Vec<Variable>,
    image: Option<ImageVariables>,
    #[serde(default, deserialize_with = "null_default")]
    services: Vec<ImageVariables>,
    #[serde(default, deserialize_with = "null_default")]
    credentials: Vec<Credential>,
    #[serde(default, deserialize_with = "null_default")]
    dependencies: Vec<Dependency>,
}
#[derive(Deserialize)]
struct Project {
    project_id: u64,
}
#[derive(Deserialize)]
struct ImageVariables {
    #[serde(default, deserialize_with = "null_default")]
    variables: Vec<Variable>,
}
#[derive(Deserialize)]
struct Credential {
    username: String,
    password: String,
}
#[derive(Deserialize)]
struct Dependency {
    token: String,
}

impl Context {
    // Preserve ownership before parsing unrelated declarations or masking metadata.
    pub fn identity(value: &Value, project: u64) -> Result<Self> {
        let id = value
            .get("id")
            .and_then(Value::as_u64)
            .filter(|id| *id > 0)
            .ok_or(Failure::Protocol("missing GitLab job cleanup identity"))?;
        Ok(Self {
            id,
            project_id: value
                .pointer("/job_info/project_id")
                .and_then(Value::as_u64)
                .unwrap_or(project),
            token: value
                .get("token")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            redactor: Redactor::default(),
        })
    }
    pub fn decode(value: &Value, admin: &str, runner: &str) -> Result<Self> {
        let mut text_bytes = preparation::validate(value)?;
        let wire: Envelope = serde_json::from_value(value.clone())
            .map_err(|_| Failure::Protocol("malformed GitLab reporting envelope"))?;
        if wire.id == 0 || wire.job_info.project_id == 0 || wire.token.is_empty() {
            return Err(Failure::Protocol("invalid GitLab reporting identity"));
        }
        let mut redactor = Redactor::default();
        for secret in [admin, runner, wire.token.as_str()] {
            redactor.add(secret)?;
        }
        // Sensitivity flags are meaningful only in the protocol's variable collections.
        let image_variables = wire
            .image
            .iter()
            .chain(&wire.services)
            .flat_map(|image| &image.variables);
        for var in wire.variables.iter().chain(image_variables) {
            if (var.masked || !var.public || var.file)
                && let Some(value) = &var.value
            {
                redactor.add(value)?;
            }
        }
        for credential in &wire.credentials {
            redactor.add(&credential.username)?;
            redactor.add(&credential.password)?;
        }
        for dependency in &wire.dependencies {
            redactor.add(&dependency.token)?;
        }
        // Also catch credentials in embedded URLs and native run's JSON string, without
        // resolving any URL or allowing it to select a reporting endpoint.
        let urls = Regex::new(r#"[a-zA-Z][a-zA-Z0-9+.-]*://([^\s/"<>@]+)@"#)
            .map_err(|_| Failure::Protocol("construct URL masking pattern"))?;
        collect(value, &urls, &mut redactor, 0)?;
        // Native run is a JSON document carried inside a JSON string. Discover
        // credentials after decoding its escapes, just as the renderer does.
        if let Some(run) = value
            .get("run")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let native: Value = serde_json::from_str(run)
                .map_err(|_| Failure::Protocol("malformed GitLab run declaration"))?;
            text_bytes = text_bytes.saturating_add(preparation::validate(&native)?);
            collect(&native, &urls, &mut redactor, 0)?;
        }
        redactor.check_work(text_bytes)?;
        Ok(Self {
            id: wire.id,
            project_id: wire.job_info.project_id,
            token: wire.token,
            redactor,
        })
    }
}

fn collect(value: &Value, urls: &Regex, redactor: &mut Redactor, depth: usize) -> Result<()> {
    if depth > 64 {
        return Err(Failure::Protocol("GitLab job exceeds nesting limit"));
    }
    match value {
        Value::String(s) => {
            for capture in urls.captures_iter(s) {
                if let Some(auth) = capture.get(1) {
                    redactor.add(auth.as_str())?;
                    if let Some((_, password)) = auth.as_str().split_once(':') {
                        redactor.add(password)?;
                    }
                }
            }
        }
        Value::Array(values) => {
            for v in values {
                collect(v, urls, redactor, depth + 1)?;
            }
        }
        Value::Object(values) => {
            for (key, v) in values {
                collect(&Value::String(key.clone()), urls, redactor, depth + 1)?;
                collect(v, urls, redactor, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
