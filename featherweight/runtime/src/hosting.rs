//! Owned, recoverable fresh executions. Keep the supervisor and its executor alive
//! until all execution owners have joined. Host effects must be owned by their
//! operation: work started outside an operation needs its own supervisor.
use crate::{CoreWasmBlock, ExecutionUsage, RuntimeError};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use structfs_core_store::{AsyncReader, AsyncWriter, Codec, Format, Reader, Writer};
use structfs_handles::CancelToken;
use structfs_service::{CleanupSupervisor, Owner, OwnerLimits};

/// Whether a denied linear-memory growth traps or returns Wasm's failure value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrowthFailure {
    #[default]
    Trap,
    ReturnFailure,
}

/// Per-run ceilings. A memory limit can tighten, never relax, its engine ceiling.
#[derive(Clone, Debug, Default)]
pub struct ExecutionPolicy {
    pub fuel: Option<u64>,
    pub memory_bytes: Option<usize>,
    pub deadline: Option<tokio::time::Instant>,
    pub growth_failure: GrowthFailure,
}
impl ExecutionPolicy {
    pub(crate) fn ensure_active(&self, cancel: &CancelToken) -> crate::Result<()> {
        if cancel.is_cancelled() {
            return Err(structfs_core_store::Error::cancelled("execution cancelled").into());
        }
        if self
            .deadline
            .is_some_and(|d| tokio::time::Instant::now() >= d)
        {
            return Err(structfs_core_store::Error::deadline_exceeded(
                "execution deadline exceeded",
            )
            .into());
        }
        Ok(())
    }
}

/// Execution failure never hides the host state. A panicked host may be inspected,
/// but `host_panicked` means its invariants are not guaranteed for another turn.
pub struct ExecutionOutcome<S> {
    pub result: crate::Result<i32>,
    pub host: S,
    pub usage: ExecutionUsage,
    pub host_panicked: bool,
}
/// Admission failure returns ownership without starting guest code.
pub struct ExecutionStartError<S> {
    pub error: RuntimeError,
    pub host: S,
}
impl<S> std::fmt::Debug for ExecutionStartError<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionStartError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Unique recoverable execution. Dropping a wait does not cancel execution.
/// Dropping this owner requests cancellation; the supplied supervisor retains
/// unfinished work and its admission until it ends. Join returns state once.
pub struct ExecutionOwner<S> {
    owner: Owner,
    result: tokio::sync::oneshot::Receiver<ExecutionOutcome<S>>,
    joined: bool,
    outcome: Option<ExecutionOutcome<S>>,
}
impl<S> ExecutionOwner<S> {
    pub fn cancel(&self) {
        self.owner.cancel();
    }
    pub fn cancellation(&self) -> CancelToken {
        self.owner.handle().cancellation()
    }
    pub fn report(&self) -> structfs_service::CloseReport {
        self.owner.handle().report()
    }
    pub async fn join(&mut self) -> crate::Result<ExecutionOutcome<S>> {
        if self.joined {
            return Err(RuntimeError::wasm(
                "join",
                "execution state already recovered",
            ));
        }
        if self.outcome.is_none() {
            self.outcome = Some((&mut self.result).await.map_err(|_| {
                RuntimeError::wasm("join", "execution task failed without recovery")
            })?);
        }
        // Cache the outcome before another await: dropping this wait must not
        // consume state or race reuse of the supervisor's capacity.
        self.owner.join().await;
        self.joined = true;
        Ok(self.outcome.take().expect("outcome received"))
    }
    /// Timeout leaves this owner and the result available for a subsequent join.
    pub async fn wait(&mut self, timeout: Duration) -> crate::Result<Option<ExecutionOutcome<S>>> {
        match tokio::time::timeout(timeout, self.join()).await {
            Ok(r) => r.map(Some),
            Err(_) => Ok(None),
        }
    }
}

fn launch<S, F, Fut>(
    supervisor: &CleanupSupervisor,
    host: S,
    run: F,
) -> Result<ExecutionOwner<S>, ExecutionStartError<S>>
where
    S: Send + 'static,
    F: FnOnce(S, CancelToken) -> Fut + Send + 'static,
    Fut: Future<Output = ExecutionOutcome<S>> + Send + 'static,
{
    let owner = match supervisor.owner(OwnerLimits {
        resources: 1,
        retained_bytes: 0,
    }) {
        Ok(owner) => owner,
        Err(error) => {
            return Err(ExecutionStartError {
                error: error.into(),
                host,
            })
        }
    };
    // Preserve ownership even if task admission rejects its closure.
    let host = Arc::new(Mutex::new(Some(host)));
    let dispatched = host.clone();
    let (send, result) = tokio::sync::oneshot::channel();
    if let Err(error) = owner.handle().spawn(move |cancel| async move {
        let host = dispatched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .expect("single dispatch");
        let outcome = run(host, cancel).await;
        // Sending to a dropped receiver drops state only AFTER owned work ends.
        let _ = send.send(outcome);
        Ok(())
    }) {
        return Err(ExecutionStartError {
            error: error.into(),
            host: host
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .expect("not dispatched"),
        });
    }
    Ok(ExecutionOwner {
        owner,
        result,
        joined: false,
        outcome: None,
    })
}

impl CoreWasmBlock {
    /// Prepared code with synchronous host effects. The entire run executes on a
    /// bounded blocking worker; accepted effects complete before recovery.
    pub fn start_sync<S, C>(
        self: &Arc<Self>,
        supervisor: &CleanupSupervisor,
        host: S,
        codec: C,
        format: Format,
        policy: ExecutionPolicy,
    ) -> Result<ExecutionOwner<S>, ExecutionStartError<S>>
    where
        S: Reader + Writer + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let block = self.clone();
        launch(supervisor, host, move |host, cancel| async move {
            block
                .run_host_sync(host, codec, format, policy, cancel)
                .await
        })
    }
    /// Prepared code with async host effects. Cancellation prevents subsequent
    /// dispatch; an outstanding host future is allowed to finish before recovery.
    /// A noncooperative future remains visible in the supervisor indefinitely.
    pub fn start_async<S, C>(
        self: &Arc<Self>,
        supervisor: &CleanupSupervisor,
        host: S,
        codec: C,
        format: Format,
        policy: ExecutionPolicy,
    ) -> Result<ExecutionOwner<S>, ExecutionStartError<S>>
    where
        S: AsyncReader + AsyncWriter + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let block = self.clone();
        launch(supervisor, host, move |host, cancel| async move {
            block
                .run_host_async(host, codec, format, policy, cancel)
                .await
        })
    }
}
