//! Shared ownership, supervision, outcomes, and cleanup policy.
use crate::{
    RunSummary,
    error::{Failure, Result},
};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success,
    Failed,
    Cancelled,
}

impl RunSummary {
    pub(crate) fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Success => self.jobs_inspected += 1,
            Outcome::Failed => self.jobs_failed += 1,
            Outcome::Cancelled => self.jobs_cancelled += 1,
        }
    }
}

pub(crate) fn combine<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Err(cleanup)) => {
            eprintln!("carcinize: cleanup also failed: {cleanup}");
            Err(error)
        }
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

/// Drain an operation that may already have acquired server-side ownership.
pub(crate) async fn acquire<T>(
    operation: impl Future<Output = Result<T>>,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> (Result<T>, Option<std::io::Result<()>>) {
    tokio::pin!(operation, shutdown);
    tokio::select! {
        result = &mut operation => (result, None),
        signal = &mut shutdown => (operation.await, Some(signal)),
    }
}

/// Only callers that know an operation is idempotent should supply retry eligibility.
pub(crate) async fn retry<T, F: Future<Output = Result<T>>>(
    mut operation: impl FnMut() -> F,
    retryable: impl Fn(&Failure) -> bool,
    retries: u32,
    delay: Duration,
) -> Result<T> {
    let mut attempt = 0;
    loop {
        match operation().await {
            Err(error) if attempt < retries && retryable(&error) => {
                attempt += 1;
                tokio::time::sleep(delay * attempt).await;
            }
            result => return result,
        }
    }
}

pub(crate) struct Supervisor {
    pub signal: Option<std::io::Result<()>>,
    deadline: Option<Instant>,
}
impl Supervisor {
    pub fn new(signal: Option<std::io::Result<()>>, deadline: Option<Instant>) -> Self {
        Self { signal, deadline }
    }

    pub async fn interrupt<F: Future<Output = std::io::Result<()>>>(
        &mut self,
        liveness: impl Future<Output = Result<()>>,
        shutdown: Pin<&mut F>,
    ) -> Failure {
        if self.signal.is_some() {
            return Failure::Cancelled;
        }
        self.observe(liveness, shutdown).await
    }

    async fn observe<F: Future<Output = std::io::Result<()>>>(
        &mut self,
        liveness: impl Future<Output = Result<()>>,
        shutdown: Pin<&mut F>,
    ) -> Failure {
        let signal = async {
            if self.signal.is_some() {
                std::future::pending().await
            } else {
                shutdown.await
            }
        };
        let deadline = async {
            match self.deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            signal = signal => { self.signal = Some(signal); Failure::Cancelled }
            result = liveness => result.err().unwrap_or(Failure::Protocol("job liveness monitor ended")),
            _ = deadline => Failure::Protocol("inspection timed out"),
        }
    }

    pub async fn run<T, F: Future<Output = std::io::Result<()>>>(
        &mut self,
        work: impl Future<Output = Result<T>>,
        liveness: impl Future<Output = Result<()>>,
        shutdown: Pin<&mut F>,
    ) -> Result<T> {
        tokio::select! {
            biased;
            error = self.interrupt(liveness, shutdown) => Err(error),
            result = work => result,
        }
    }

    /// Keep an in-flight final write owned even if supervision interrupts it.
    /// Callers retain provider-specific responsibility for changing/confirming its outcome.
    pub async fn drain<T, F: Future<Output = std::io::Result<()>>>(
        &mut self,
        work: impl Future<Output = Result<T>>,
        liveness: impl Future<Output = Result<()>>,
        mut shutdown: Pin<&mut F>,
    ) -> (Result<T>, Option<Failure>) {
        tokio::pin!(work, liveness);
        // Begin/poll the owned write before observing a new interruption. Once
        // terminal submission has begun, its acknowledgment decides the outcome.
        let mut interruption = None;
        loop {
            let already_stopping = self.signal.is_some();
            tokio::select! {
                biased;
                result = &mut work => return (result, interruption),
                error = self.observe(liveness.as_mut(), shutdown.as_mut()) => {
                    interruption.get_or_insert(error);
                    if !already_stopping && self.signal.is_some() {
                        // A local stop does not terminate remote liveness. Keep polling
                        // it while the already-started write drains.
                        continue;
                    }
                    return (work.await, interruption);
                }
            }
        }
    }

    /// Preserve operational shutdown errors after owned remote operations are drained.
    pub fn finish<T>(self, result: Result<T>) -> Result<(T, bool)> {
        let stop = self.signal.is_some();
        if let Some(signal) = self.signal {
            signal?;
        }
        result.map(|value| (value, stop))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn final_write_is_drained_and_liveness_continues_after_shutdown_error() {
        let signaled = Arc::new(AtomicBool::new(false));
        let flag = signaled.clone();
        let shutdown = async move {
            flag.store(true, Ordering::SeqCst);
            Err(std::io::Error::other("synthetic signal failure"))
        };
        tokio::pin!(shutdown);
        let (ready, received) = tokio::sync::oneshot::channel();
        let liveness = async {
            assert!(signaled.load(Ordering::SeqCst));
            ready.send(()).expect("work still owned");
            std::future::pending::<Result<()>>().await
        };
        let work = async {
            received.await.expect("liveness continued");
            Ok(42)
        };
        let mut supervisor = Supervisor::new(None, None);
        let (result, interruption) = supervisor.drain(work, liveness, shutdown.as_mut()).await;
        assert_eq!(result.expect("drained write"), 42);
        assert!(matches!(interruption, Some(Failure::Cancelled)));
        assert!(matches!(supervisor.finish(Ok(())), Err(Failure::Signal(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_interrupts_work_and_retry_budget_is_finite() {
        let shutdown = std::future::pending::<std::io::Result<()>>();
        tokio::pin!(shutdown);
        let mut supervisor = Supervisor::new(None, Some(Instant::now() + Duration::from_secs(1)));
        let result: Result<()> = supervisor
            .run(
                std::future::pending(),
                std::future::pending(),
                shutdown.as_mut(),
            )
            .await;
        assert!(matches!(
            result,
            Err(Failure::Protocol("inspection timed out"))
        ));
        let attempts = std::cell::Cell::new(0);
        let result: Result<()> = retry(
            || {
                attempts.set(attempts.get() + 1);
                std::future::ready(Err(Failure::Http {
                    operation: "delete",
                    status: 503,
                }))
            },
            Failure::transient,
            3,
            Duration::from_secs(2),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.get(), 4);
    }

    #[tokio::test]
    async fn registration_ownership_is_returned_even_if_shutdown_arrives_first() {
        let (send, receive) = tokio::sync::oneshot::channel();
        let operation = async { Ok(receive.await.expect("identity")) };
        let shutdown = async {
            send.send(42).expect("operation retained");
            Ok(())
        };
        let (result, signal) = acquire(operation, shutdown).await;
        assert_eq!(result.expect("registration"), 42);
        assert!(matches!(signal, Some(Ok(()))));
    }
}
