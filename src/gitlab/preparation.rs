//! Bounded pure preparation, invoked on one owned blocking worker per assignment.
use super::{
    context::Context,
    model::Job,
    trace::{self, Trace},
};
use crate::error::{Failure, Result};
use serde_json::Value;
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) fn validate(value: &Value) -> Result<usize> {
    fn visit(value: &Value, depth: usize, nodes: &mut usize, bytes: &mut usize) -> Result<()> {
        *nodes += 1;
        if depth > 48 || *nodes > 8192 {
            return Err(Failure::Protocol(
                "GitLab declaration exceeds preparation structure budget",
            ));
        }
        match value {
            Value::String(s) => *bytes = bytes.saturating_add(s.len()),
            Value::Number(n) => *bytes = bytes.saturating_add(n.to_string().len()),
            Value::Array(items) => {
                for item in items {
                    visit(item, depth + 1, nodes, bytes)?;
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    *bytes = bytes.saturating_add(key.len());
                    visit(item, depth + 1, nodes, bytes)?;
                }
            }
            _ => {}
        }
        if *bytes > crate::http::MAX_BODY {
            return Err(Failure::Protocol(
                "GitLab declaration exceeds 8 MiB preparation budget",
            ));
        }
        Ok(())
    }
    let mut nodes = 0;
    let mut bytes = 0;
    visit(value, 0, &mut nodes, &mut bytes)?;
    Ok(bytes.saturating_add(nodes * 32))
}

pub(super) fn prepare(
    payload: Value,
    admin: &str,
    runner: &str,
    canceled: &AtomicBool,
) -> Result<Trace> {
    let checkpoint = || {
        if canceled.load(Ordering::Relaxed) {
            Err(Failure::Cancelled)
        } else {
            Ok(())
        }
    };
    checkpoint()?;
    validate(&payload)?;
    let context = Context::decode(&payload, admin, runner)?;
    checkpoint()?;
    let job = Job::decode(payload)?;
    checkpoint()?;
    trace::render(&job, &context.redactor, chrono::Utc::now().timestamp())
}

// The caller supplies the cancellation/deadline/heartbeat policy. The worker is
// always joined, including when that policy interrupts preparation.
pub(super) async fn owned(
    work: impl FnOnce(Arc<AtomicBool>) -> Result<Trace> + Send + 'static,
    interrupt: impl Future<Output = Failure>,
) -> Result<Trace> {
    let canceled = Arc::new(AtomicBool::new(false));
    let worker_canceled = canceled.clone();
    let mut worker = tokio::task::spawn_blocking(move || work(worker_canceled));
    tokio::select! {
        biased;
        error = interrupt => {
            canceled.store(true, Ordering::Relaxed);
            let _ = worker.await?;
            Err(error)
        }
        result = &mut worker => result?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn busy_preparation_does_not_block_interrupt_and_is_joined_before_returning() {
        let (entered, started) = tokio::sync::oneshot::channel();
        let joined = Arc::new(AtomicBool::new(false));
        let finished = joined.clone();
        let result = owned(
            move |canceled| {
                entered.send(()).expect("entered");
                while !canceled.load(Ordering::Relaxed) {
                    std::thread::yield_now();
                }
                finished.store(true, Ordering::Relaxed);
                Err(Failure::Cancelled)
            },
            async {
                started.await.expect("worker started");
                // This executes on the only async runtime thread while the CPU worker
                // remains busy, modeling shutdown, heartbeat, or deadline readiness.
                Failure::Cancelled
            },
        )
        .await;
        assert!(matches!(result, Err(Failure::Cancelled)));
        assert!(joined.load(Ordering::Relaxed));
    }
}
