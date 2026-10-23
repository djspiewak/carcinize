use super::model::Job;
use crate::{
    error::{Failure, Result},
    inspection::{self, Bounded, MAX_OUTPUT},
    redact::{Masker, Redactor},
};
use serde::Serialize;
use serde_json::json;
use std::io::Write;

use crate::inspection::BANNER;

pub(super) struct Trace {
    pub bytes: Vec<u8>,
    pub sections: Vec<usize>,
}

#[cfg(test)]
pub(super) fn render(job: &Job, redactor: &Redactor, timestamp: i64) -> Result<Trace> {
    render_with(job, &mut redactor.masker(&|| Ok(())), timestamp)
}

pub(super) fn render_with(job: &Job, masker: &mut Masker<'_>, timestamp: i64) -> Result<Trace> {
    masker.checkpoint()?;
    let header = json!({"id":job.id,"job_info":job.job_info,"git_info":job.git_info,
        "runner_info":job.runner_info,"allow_git_fetch":job.allow_git_fetch,"variables":job.variables,
        "image":job.image,"services":job.services,"artifacts":job.artifacts,"cache":job.cache,
        "dependencies":job.dependencies});
    let mut output = Bounded::new(MAX_OUTPUT);
    output.write_all(BANNER.as_bytes()).map_err(|_| limit())?;
    let mut sections = Vec::new();
    section(
        &mut output,
        &mut sections,
        masker,
        timestamp,
        "Job declaration",
        &header,
    )?;
    for hook in &job.hooks {
        section(
            &mut output,
            &mut sections,
            masker,
            timestamp,
            "Hook declaration",
            hook,
        )?;
    }
    for step in &job.steps {
        section(
            &mut output,
            &mut sections,
            masker,
            timestamp,
            "Script declaration",
            step,
        )?;
    }
    for step in &job.native_steps {
        section(
            &mut output,
            &mut sections,
            masker,
            timestamp,
            "Native step declaration",
            step,
        )?;
    }
    Ok(Trace {
        bytes: output.bytes,
        sections,
    })
}

fn section<T: Serialize>(
    output: &mut Bounded,
    sections: &mut Vec<usize>,
    masker: &mut Masker<'_>,
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
    inspection::render(output, value, masker)?;
    writeln!(
        output,
        "\n\x1b[0Ksection_end:{timestamp}:carcinize_{index}\r\x1b[0K"
    )
    .map_err(|_| limit())?;
    sections.push(output.bytes.len());
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
