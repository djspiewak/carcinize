//! The inspected declarations, deliberately independent of transport credentials.
use crate::github::protocol_enum::ProtocolEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod template;
pub(crate) use template::{TemplateValue, decode_template};

#[derive(Debug, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("unsupported {0}")]
    Unsupported(&'static str),
}

#[derive(Debug, Serialize)]
pub(crate) struct Job {
    pub id: Uuid,
    pub name: String,
    pub display_name: String,
    pub environment: Vec<TemplateValue>,
    pub defaults: Vec<TemplateValue>,
    pub container: Option<TemplateValue>,
    pub services: Option<TemplateValue>,
    pub outputs: Option<TemplateValue>,
    pub file_table: Vec<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Step {
    pub id: Uuid,
    pub name: Option<String>,
    pub context_name: Option<String>,
    pub display_name: Option<TemplateValue>,
    pub condition: Option<String>,
    pub enabled: bool,
    pub environment: Option<TemplateValue>,
    pub continue_on_error: Option<TemplateValue>,
    pub timeout_minutes: Option<TemplateValue>,
    pub background: bool,
    pub parallel_group_id: Option<String>,
    #[serde(flatten)]
    pub kind: StepKind,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum StepKind {
    Script {
        script: TemplateValue,
        shell: Option<TemplateValue>,
        working_directory: Option<TemplateValue>,
    },
    RepositoryAction {
        repository: String,
        reference: String,
        path: String,
        inputs: Option<TemplateValue>,
    },
    LocalAction {
        path: String,
        inputs: Option<TemplateValue>,
    },
    ContainerAction {
        image: String,
        inputs: Option<TemplateValue>,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireJob {
    job_id: Uuid,
    job_name: String,
    job_display_name: String,
    #[serde(default)]
    environment_variables: Vec<Value>,
    #[serde(default)]
    defaults: Vec<Value>,
    job_container: Option<Value>,
    job_service_containers: Option<Value>,
    job_outputs: Option<Value>,
    #[serde(default)]
    file_table: Vec<String>,
    steps: Vec<Value>,
}

impl Job {
    pub fn decode(value: Value) -> Result<Self, DecodeError> {
        let wire: WireJob =
            serde_json::from_value(value).map_err(|_| DecodeError::Malformed("job declaration"))?;
        let steps = wire
            .steps
            .into_iter()
            .map(decode_step)
            .collect::<Result<Vec<_>, _>>()?;
        let mut ids = std::collections::HashSet::new();
        if steps.iter().any(|s| s.id.is_nil() || !ids.insert(s.id)) {
            return Err(DecodeError::Malformed("duplicate or empty step identity"));
        }
        Ok(Self {
            id: wire.job_id,
            name: wire.job_name,
            display_name: wire.job_display_name,
            environment: wire
                .environment_variables
                .into_iter()
                .map(decode_template)
                .collect::<Result<_, _>>()?,
            defaults: wire
                .defaults
                .into_iter()
                .map(decode_template)
                .collect::<Result<_, _>>()?,
            container: optional_template(wire.job_container)?,
            services: optional_template(wire.job_service_containers)?,
            outputs: optional_template(wire.job_outputs)?,
            file_table: wire.file_table,
            steps,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireStep {
    #[serde(rename = "type")]
    step_type: ProtocolEnum,
    id: Uuid,
    name: Option<String>,
    context_name: Option<String>,
    display_name: Option<String>,
    display_name_token: Option<Value>,
    condition: Option<String>,
    #[serde(default = "enabled")]
    enabled: bool,
    environment: Option<Value>,
    continue_on_error: Option<Value>,
    timeout_in_minutes: Option<Value>,
    #[serde(default)]
    background: bool,
    parallel_group_id: Option<String>,
    reference: WireReference,
    inputs: Option<Value>,
}

fn enabled() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireReference {
    #[serde(rename = "type")]
    source_type: ProtocolEnum,
    repository_type: Option<String>,
    name: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
    #[serde(default)]
    path: String,
    image: Option<String>,
}

pub(crate) fn decode_step(value: Value) -> Result<Step, DecodeError> {
    let wire: WireStep =
        serde_json::from_value(value).map_err(|_| DecodeError::Malformed("step declaration"))?;
    if wire.step_type.code(&[(4, "action")]).is_none() {
        return Err(DecodeError::Unsupported("step type"));
    }
    let inputs = optional_template(wire.inputs)?;
    let kind = match wire.reference.source_type.code(&[
        (1, "repository"),
        (2, "containerRegistry"),
        (3, "script"),
    ]) {
        Some(1) => match wire.reference.repository_type.as_deref() {
            Some(t) if t.eq_ignore_ascii_case("self") => StepKind::LocalAction {
                path: wire.reference.path,
                inputs,
            },
            Some(t) if t.eq_ignore_ascii_case("github") => StepKind::RepositoryAction {
                repository: wire
                    .reference
                    .name
                    .filter(|s| !s.is_empty())
                    .ok_or(DecodeError::Malformed("action repository"))?,
                reference: wire
                    .reference
                    .reference
                    .filter(|s| !s.is_empty())
                    .ok_or(DecodeError::Malformed("action ref"))?,
                path: wire.reference.path,
                inputs,
            },
            _ => return Err(DecodeError::Unsupported("action repository type")),
        },
        Some(2) => StepKind::ContainerAction {
            image: wire
                .reference
                .image
                .filter(|s| !s.is_empty())
                .ok_or(DecodeError::Malformed("container image"))?,
            inputs,
        },
        Some(3) => {
            let fields = inputs
                .as_ref()
                .ok_or(DecodeError::Malformed("script inputs"))?;
            fields.validate_script_keys()?;
            StepKind::Script {
                script: fields
                    .lookup("script")?
                    .cloned()
                    .ok_or(DecodeError::Malformed("script body"))?,
                shell: fields.lookup("shell")?.cloned(),
                working_directory: fields.lookup("workingDirectory")?.cloned(),
            }
        }
        _ => return Err(DecodeError::Unsupported("action reference type")),
    };
    Ok(Step {
        id: wire.id,
        name: wire.name,
        context_name: wire.context_name,
        display_name: optional_template(
            wire.display_name_token
                .or(wire.display_name.map(Value::String)),
        )?,
        condition: wire.condition,
        enabled: wire.enabled,
        environment: optional_template(wire.environment)?,
        continue_on_error: optional_template(wire.continue_on_error)?,
        timeout_minutes: optional_template(wire.timeout_in_minutes)?,
        background: wire.background,
        parallel_group_id: wire.parallel_group_id,
        kind,
    })
}

fn optional_template(value: Option<Value>) -> Result<Option<TemplateValue>, DecodeError> {
    value.map(decode_template).transpose()
}

impl Step {
    pub fn label(&self) -> String {
        self.display_name
            .as_ref()
            .and_then(TemplateValue::as_literal)
            .map(str::to_owned)
            .or_else(|| self.context_name.clone())
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| format!("Inspect step {}", self.id))
    }
}

#[cfg(test)]
mod tests;
