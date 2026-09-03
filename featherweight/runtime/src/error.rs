//! Runtime error types.

use thiserror::Error;

/// Errors from the Featherweight runtime.
#[derive(Debug, Error)]
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
}

impl RuntimeError {
    /// Create an assembly-definition error.
    pub fn assembly(message: impl Into<String>) -> Self {
        RuntimeError::Assembly(message.into())
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
