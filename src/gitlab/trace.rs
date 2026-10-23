use super::model::Job;
use crate::{
    error::{Failure, Result},
    http::MAX_BODY,
    redact::Redactor,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Write;

pub(super) const BANNER: &str =
    "CARCINIZE INSPECTION ONLY: declarations are described; nothing is executed or resolved.\n";

pub(super) struct Trace {
    pub bytes: Vec<u8>,
    pub sections: Vec<usize>,
}

struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_BODY.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("inspection trace exceeds 8 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn render(job: &Job, redactor: &Redactor, timestamp: i64) -> Result<Trace> {
    let header = json!({"id":job.id,"job_info":job.job_info,"git_info":job.git_info,
        "runner_info":job.runner_info,"allow_git_fetch":job.allow_git_fetch,"variables":job.variables,
        "image":job.image,"services":job.services,"artifacts":job.artifacts,"cache":job.cache,
        "dependencies":job.dependencies});
    let mut output = Bounded(Vec::new());
    output.write_all(BANNER.as_bytes()).map_err(|_| limit())?;
    let mut sections = Vec::new();
    section(
        &mut output,
        &mut sections,
        redactor,
        timestamp,
        "Job declaration",
        &header,
    )?;
    for hook in &job.hooks {
        section(
            &mut output,
            &mut sections,
            redactor,
            timestamp,
            "Hook declaration",
            hook,
        )?;
    }
    for step in &job.steps {
        section(
            &mut output,
            &mut sections,
            redactor,
            timestamp,
            "Script declaration",
            step,
        )?;
    }
    for step in &job.native_steps {
        section(
            &mut output,
            &mut sections,
            redactor,
            timestamp,
            "Native step declaration",
            step,
        )?;
    }
    Ok(Trace {
        bytes: output.0,
        sections,
    })
}

fn section<T: Serialize>(
    output: &mut Bounded,
    sections: &mut Vec<usize>,
    redactor: &Redactor,
    timestamp: i64,
    label: &str,
    value: &T,
) -> Result<()> {
    let index = sections.len();
    // IDs and titles are generated locally so declarations cannot forge terminal controls.
    writeln!(
        output,
        "\x1b[0Ksection_start:{timestamp}:carcinize_{index}\r\x1b[0K{label}"
    )
    .map_err(|_| limit())?;
    let mut value =
        serde_json::to_value(value).map_err(|_| Failure::Protocol("render GitLab declaration"))?;
    mask(&mut value, redactor)?;
    serde_json::to_writer_pretty(&mut *output, &value).map_err(|_| limit())?;
    writeln!(
        output,
        "\n\x1b[0Ksection_end:{timestamp}:carcinize_{index}\r\x1b[0K"
    )
    .map_err(|_| limit())?;
    sections.push(output.0.len());
    Ok(())
}

fn mask(value: &mut Value, redactor: &Redactor) -> Result<()> {
    match value {
        Value::String(s) => *s = redactor.redact(s),
        Value::Array(items) => {
            for value in items {
                mask(value, redactor)?;
            }
        }
        Value::Object(values) => {
            let old = std::mem::take(values);
            for (key, mut value) in old {
                mask(&mut value, redactor)?;
                if values.insert(redactor.redact(&key), value).is_some() {
                    return Err(Failure::Protocol(
                        "masking GitLab declaration keys would lose fields",
                    ));
                }
            }
        }
        scalar => {
            let original = scalar.to_string();
            let masked = redactor.redact(&original);
            if masked != original {
                *scalar = Value::String(masked);
            }
        }
    }
    Ok(())
}

fn limit() -> Failure {
    Failure::Protocol("GitLab inspection trace exceeds 8 MiB or cannot be rendered")
}

pub(super) fn diagnostic(error: &Failure, redactor: &Redactor) -> Trace {
    let bytes = format!(
        "{BANNER}Inspection failed: {}\n",
        redactor.redact(&error.to_string())
    )
    .into_bytes();
    let sections = vec![bytes.len()];
    Trace { bytes, sections }
}
