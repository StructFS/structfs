//! Error types for the Featherweight runtime.

use thiserror::Error;
use uuid::Uuid;

/// Errors that can occur in the Featherweight runtime.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// A Block with the given ID was not found.
    #[error("block not found: {0}")]
    BlockNotFound(Uuid),

    /// The Block has already been started.
    #[error("block already running: {0}")]
    BlockAlreadyRunning(Uuid),

    /// The Block has already been stopped.
    #[error("block already stopped: {0}")]
    BlockAlreadyStopped(Uuid),

    /// A store operation failed.
    #[error("store error: {0}")]
    Store(#[from] structfs_core_store::Error),

    /// The channel was closed unexpectedly.
    #[error("channel closed")]
    ChannelClosed,

    /// An I/O error occurred.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A path was invalid.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// The export was not found.
    #[error("export not found: {0}")]
    ExportNotFound(String),
}

/// Result type alias for runtime operations.
pub type Result<T> = std::result::Result<T, RuntimeError>;
