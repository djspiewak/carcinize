//! GitHub declaration preparation; transport consumes only masked, bounded records.
use super::{job_context::JobContext, model::Job};
use crate::{
    error::{Failure, Result},
    inspection::{self, BANNER, Bounded, MAX_OUTPUT},
    redact::Redactor,
};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

pub(super) struct Step {
    pub id: Uuid,
    pub label: String,
    pub lines: Vec<String>,
    pub log: String,
}

pub(super) struct Prepared {
    pub context: JobContext,
    pub redactor: Redactor,
    pub steps: Vec<Step>,
}

pub(super) fn prepare(
    payload: Value,
    admin: &str,
    canceled: &AtomicBool,
    timestamp: &str,
) -> Result<Prepared> {
    let checkpoint = || {
        if canceled.load(Ordering::Relaxed) {
            Err(Failure::Cancelled)
        } else {
            Ok(())
        }
    };
    checkpoint()?;
    let bytes = crate::preparation::validate(&payload)?;
    let context = JobContext::decode(&payload)?;
    let redactor = context.redactor(admin)?;
    redactor.check_work(bytes)?;
    checkpoint()?;
    let job = Job::decode(payload)?;
    let mut masker = redactor.masker(&checkpoint);
    let mut remaining = MAX_OUTPUT;
    let mut steps = Vec::new();
    for (index, step) in job.steps.iter().enumerate() {
        checkpoint()?;
        let mut text = Bounded::new(remaining);
        text.write_all(BANNER.as_bytes()).map_err(|_| limit())?;
        if index == 0 {
            let header = json!({"job":job.name,"display_name":job.display_name,"environment":job.environment,
                "defaults":job.defaults,"container":job.container,"services":job.services,"outputs":job.outputs,
                "file_table":job.file_table});
            inspection::render(&mut text, &header, &mut masker)?;
            text.write_all(b"\n").map_err(|_| limit())?;
        }
        inspection::render(&mut text, step, &mut masker)?;
        let text = String::from_utf8(text.bytes)
            .map_err(|_| Failure::Protocol("invalid declaration text"))?;
        let mut log = Bounded::new(remaining);
        let mut lines = Vec::new();
        for line in text.lines() {
            let mut chars = line.chars().peekable();
            // Also preserve an empty line as one prefixed output record.
            loop {
                let chunk: String = chars.by_ref().take(1000).collect();
                let line = if chunk.is_empty() {
                    "carcinize |".to_owned()
                } else {
                    format!("carcinize | {chunk}")
                };
                writeln!(log, "{timestamp} {line}").map_err(|_| limit())?;
                lines.push(line);
                if chars.peek().is_none() {
                    break;
                }
            }
        }
        remaining -= log.bytes.len();
        let label = step.label();
        redactor.check_work(label.len())?;
        steps.push(Step {
            id: step.id,
            label: masker.redact(&label)?,
            lines,
            log: String::from_utf8(log.bytes).map_err(|_| Failure::Protocol("invalid log text"))?,
        });
    }
    Ok(Prepared {
        context,
        redactor,
        steps,
    })
}

fn limit() -> Failure {
    Failure::Protocol("inspection logs exceed 8 MiB")
}
