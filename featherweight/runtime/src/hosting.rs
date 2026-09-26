//! Owned, recoverable fresh executions. Keep the supervisor and its executor alive
//! until all execution owners have joined. Host effects must be owned by their
//! operation: work started outside an operation needs its own supervisor.
use crate::core_wasm::HostRun;
use crate::{CoreWasmBlock, ExecutionMeter, ExecutionUsage, RuntimeError};
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
#[non_exhaustive]
pub enum GrowthFailure {
    #[default]
    Trap,
    ReturnFailure,
}

/// Per-run ceilings. A memory limit can tighten, never relax, its engine ceiling.
///
/// Build with [`ExecutionPolicy::default`] and the `with_*` methods.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ExecutionPolicy {
    /// Wasmtime fuel for the run; `None` is unbounded (still counted).
    pub fuel: Option<u64>,
    /// Linear-memory ceiling in bytes; `None` uses the engine ceiling.
    pub memory_bytes: Option<usize>,
    /// Absolute deadline for admission and execution.
    pub deadline: Option<tokio::time::Instant>,
    /// What a denied memory growth does.
    pub growth_failure: GrowthFailure,
}

impl ExecutionPolicy {
    /// Cap the run's fuel.
    pub fn with_fuel(mut self, fuel: u64) -> Self {
        self.fuel = Some(fuel);
        self
    }

    /// Tighten the linear-memory ceiling.
    pub fn with_memory_bytes(mut self, bytes: usize) -> Self {
        self.memory_bytes = Some(bytes);
        self
    }

    /// Bound admission and execution by an absolute deadline.
    pub fn with_deadline(mut self, deadline: tokio::time::Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Choose what a denied memory growth does.
    pub fn with_growth_failure(mut self, growth_failure: GrowthFailure) -> Self {
        self.growth_failure = growth_failure;
        self
    }

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
#[non_exhaustive]
pub struct ExecutionOutcome<S> {
    pub result: crate::Result<i32>,
    pub host: S,
    pub usage: ExecutionUsage,
    pub host_panicked: bool,
}
/// Admission failure returns ownership without starting guest code.
#[non_exhaustive]
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
    /// The host until the supervised task takes it; still here if the task
    /// never ran.
    pending: Arc<Mutex<Option<S>>>,
    joined: bool,
    outcome: Option<ExecutionOutcome<S>>,
}
impl<S> ExecutionOwner<S> {
    /// Request cleanup without blocking: close the owner, which cancels the
    /// run. [`join`](Self::join) (or [`wait`](Self::wait) with a timeout)
    /// then waits for it and returns the host state.
    pub fn close(&self) {
        self.owner.close();
    }
    pub fn cancellation(&self) -> CancelToken {
        self.owner.handle().cancellation()
    }
    pub fn report(&self) -> structfs_service::CloseReport {
        self.owner.handle().report()
    }
    /// Wait for the run and recover its host state. A second join after a
    /// successful one fails with [`RuntimeError::AlreadyJoined`]; a run
    /// whose host state was destroyed fails with [`RuntimeError::HostPanic`].
    pub async fn join(&mut self) -> crate::Result<ExecutionOutcome<S>> {
        if self.joined {
            return Err(RuntimeError::AlreadyJoined);
        }
        if self.outcome.is_none() {
            match (&mut self.result).await {
                Ok(outcome) => self.outcome = Some(outcome),
                Err(_) => {
                    // The supervised task never took the host: it never ran
                    // (its executor shut down), so nothing touched the host
                    // and there is no owned work to wait for.
                    let host = self
                        .pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take();
                    let Some(host) = host else {
                        return Err(RuntimeError::HostPanic(
                            "execution ended without recoverable host state".into(),
                        ));
                    };
                    self.joined = true;
                    return Ok(ExecutionOutcome {
                        result: Err(RuntimeError::ExecutionLost(
                            "the supervised execution task never started".into(),
                        )),
                        host,
                        usage: ExecutionUsage {
                            finished: true,
                            ..ExecutionUsage::default()
                        },
                        host_panicked: false,
                    });
                }
            }
        }
        // Cache the outcome before another await: dropping this wait must not
        // consume state or race reuse of the supervisor's capacity.
        self.owner.join_indefinitely().await;
        self.joined = true;
        self.outcome.take().ok_or(RuntimeError::AlreadyJoined)
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
    Fut: Future<Output = Option<ExecutionOutcome<S>>> + Send + 'static,
{
    let owner = match supervisor.owner(
        OwnerLimits::default()
            .with_resources(1)
            .with_retained_bytes(0),
    ) {
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
        let Some(host) = dispatched.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return Ok(());
        };
        // `None` means the host state was destroyed; the receiver then
        // reports that instead of an outcome.
        if let Some(outcome) = run(host, cancel).await {
            // Sending to a dropped receiver drops state only AFTER owned work ends.
            let _ = send.send(outcome);
        }
        Ok(())
    }) {
        return Err(ExecutionStartError {
            error: error.into(),
            host: host
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .expect("a refused spawn never dispatches its host"),
        });
    }
    Ok(ExecutionOwner {
        owner,
        result,
        pending: host,
        joined: false,
        outcome: None,
    })
}

impl CoreWasmBlock {
    /// Prepared code with synchronous host effects. The entire run executes on a
    /// bounded blocking worker of the engine's runtime; accepted effects complete
    /// before recovery.
    pub fn start_sync<S, C>(
        self: &Arc<Self>,
        supervisor: &CleanupSupervisor,
        host: S,
        codec: C,
        format: Format,
        policy: ExecutionPolicy,
    ) -> Result<ExecutionOwner<S>, ExecutionStartError<S>>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let block = self.clone();
        launch(supervisor, host, move |host, cancel| async move {
            block
                .run_host_sync(HostRun {
                    host,
                    codec,
                    format,
                    policy,
                    cancel,
                    meter: ExecutionMeter::default(),
                })
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
        S: AsyncReader + AsyncWriter + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let block = self.clone();
        launch(supervisor, host, move |host, cancel| async move {
            Some(
                block
                    .run_host_async(HostRun {
                        host,
                        codec,
                        format,
                        policy,
                        cancel,
                        meter: ExecutionMeter::default(),
                    })
                    .await,
            )
        })
    }
}
