//! GitLab declaration decoding and trace preparation.
use super::{
    context::Context,
    model::Job,
    trace::{self, Trace},
};
use crate::error::{Failure, Result};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn prepare(
    payload: Value,
    admin: &str,
    runner: &str,
    canceled: &AtomicBool,
    timestamp: i64,
) -> Result<Trace> {
    let checkpoint = || {
        if canceled.load(Ordering::Relaxed) {
            Err(Failure::Cancelled)
        } else {
            Ok(())
        }
    };
    checkpoint()?;
    let context = Context::decode(&payload, admin, runner)?;
    checkpoint()?;
    let job = Job::decode(payload)?;
    checkpoint()?;
    trace::render_with(&job, &mut context.redactor.masker(&checkpoint), timestamp)
}
