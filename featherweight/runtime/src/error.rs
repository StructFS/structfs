//! Runtime error types.

use thiserror::Error;

/// Errors from the Featherweight runtime.
///
/// Every failure class an embedder may want to branch on has its own
/// variant; `Wasm` is reserved for the engine itself (compile, link,
/// instantiate, trap) and carries the failing step for diagnostics only.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RuntimeError {
    /// A store operation failed.
    #[error("store error: {0}")]
    Store(#[from] structfs_core_store::Error),

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// An assembly definition is invalid.
    #[error("assembly error: {0}")]
    Assembly(String),

    /// The wasm engine failed (compile, link, instantiate, or trap).
    #[error("wasm error during {operation}: {message}")]
    Wasm {
        operation: &'static str,
        message: String,
    },

    /// A block's manifest is missing or malformed.
    #[error("manifest error: {0}")]
    Manifest(String),

    /// Capacity was not available: an assembly or execution reservation
    /// was refused, or the artifact is bound to another engine's capacity.
    #[error("admission refused: {0}")]
    Admission(String),

    /// A requested execution policy cannot be honoured (for example a
    /// memory limit above the engine ceiling).
    #[error("execution policy rejected: {0}")]
    Policy(String),

    /// Engine construction parameters are invalid, or the engine was
    /// created outside a Tokio runtime.
    #[error("engine configuration: {0}")]
    EngineConfig(String),

    /// An owned execution's host state was already recovered by an
    /// earlier join.
    #[error("execution state already recovered")]
    AlreadyJoined,

    /// The execution task ended without delivering its outcome (its
    /// executor shut down underneath it).
    #[error("execution lost: {0}")]
    ExecutionLost(String),

    /// Host code panicked while driving the guest. The recovered host
    /// state, if any, may have broken invariants.
    #[error("host panic: {0}")]
    HostPanic(String),
}

impl RuntimeError {
    /// Create an assembly-definition error.
    pub fn assembly(message: impl Into<String>) -> Self {
        RuntimeError::Assembly(message.into())
    }

    /// Classify a failed join of a runtime-owned task: a panic is
    /// [`RuntimeError::HostPanic`], a task cancelled by its executor
    /// shutting down is [`RuntimeError::ExecutionLost`]. `task` names it.
    pub fn task_failed(task: &str, error: tokio::task::JoinError) -> Self {
        if error.is_panic() {
            RuntimeError::HostPanic(format!("{task} panicked"))
        } else {
            RuntimeError::ExecutionLost(format!("{task} was cancelled: {error}"))
        }
    }

    /// Create a wasm-engine error.
    pub fn wasm(operation: &'static str, message: impl ToString) -> Self {
        RuntimeError::Wasm {
            operation,
            message: message.to_string(),
        }
    }
}

/// Runtime result alias.
pub type Result<T> = std::result::Result<T, RuntimeError>;

#[cfg(test)]
mod tests {
    use super::RuntimeError;

    #[tokio::test]
    async fn failed_task_joins_are_typed() {
        let panicked = tokio::spawn(async { panic!("boom") }).await.unwrap_err();
        assert!(matches!(
            RuntimeError::task_failed("probe", panicked),
            RuntimeError::HostPanic(_)
        ));
        let pending = tokio::spawn(std::future::pending::<()>());
        pending.abort();
        let cancelled = pending.await.unwrap_err();
        assert!(matches!(
            RuntimeError::task_failed("probe", cancelled),
            RuntimeError::ExecutionLost(_)
        ));
    }
}
