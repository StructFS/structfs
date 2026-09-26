//! One absolute deadline and cancellation token for a fresh execution session.
use std::{future::Future, time::Duration};
use structfs_core_store::Error;
use structfs_handles::CancelToken;
use tokio::time::Instant;

#[derive(Clone)]
pub struct ExecutionScope {
    deadline: Instant,
    cancel: CancelToken,
}
impl ExecutionScope {
    pub fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            cancel: CancelToken::new(),
        }
    }
    /// Cancel in-flight work under this scope. Not a stop verb: a scope owns
    /// no resources, so there is no cleanup to request (`close`) or wait
    /// for (`join`); every operation run under it fails `Cancelled`.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub(crate) fn cancellation(&self) -> CancelToken {
        self.cancel.clone()
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub async fn ended(&self) -> Error {
        tokio::select! { biased;
            _ = self.cancel.cancelled() => Error::cancelled("execution cancelled"),
            _ = tokio::time::sleep_until(self.deadline) => Error::deadline_exceeded("execution deadline exceeded"),
        }
    }
    pub(crate) async fn run<T>(
        &self,
        work: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        tokio::select! { biased;
            error = self.ended() => Err(error),
            result = work => result,
        }
    }
}
pub(crate) struct Watch(pub tokio::task::JoinHandle<()>);
impl Drop for Watch {
    fn drop(&mut self) {
        self.0.abort();
    }
}
