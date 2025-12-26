//! Error types for the adapter layer.

use std::fmt;

/// Errors that can occur when converting between legacy and new store types.
#[derive(Debug)]
pub enum Error {
    /// Error converting a path.
    PathConversion(String),
    /// Error from the legacy store.
    LegacyStore(structfs_store::Error),
    /// Error from the new store.
    NewStore(structfs_core_store::Error),
    /// Error during serde conversion.
    Serde(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::PathConversion(msg) => write!(f, "path conversion error: {}", msg),
            Error::LegacyStore(e) => write!(f, "legacy store error: {}", e),
            Error::NewStore(e) => write!(f, "new store error: {}", e),
            Error::Serde(msg) => write!(f, "serde error: {}", msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::LegacyStore(e) => Some(e),
            Error::NewStore(e) => Some(e),
            _ => None,
        }
    }
}

impl From<structfs_store::Error> for Error {
    fn from(e: structfs_store::Error) -> Self {
        Error::LegacyStore(e)
    }
}

impl From<structfs_core_store::Error> for Error {
    fn from(e: structfs_core_store::Error) -> Self {
        Error::NewStore(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

// Conversions to/from store error types for trait implementations

impl From<Error> for structfs_store::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::LegacyStore(e) => e,
            Error::PathConversion(msg) => {
                structfs_store::Error::PathError(structfs_store::PathError::PathStringInvalid {
                    path: String::new(),
                    message: msg,
                })
            }
            Error::NewStore(e) => structfs_store::Error::ImplementationFailure {
                message: e.to_string(),
            },
            Error::Serde(msg) => structfs_store::Error::RecordDeserialization { message: msg },
        }
    }
}

impl From<Error> for structfs_core_store::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::NewStore(e) => e,
            Error::PathConversion(msg) => {
                structfs_core_store::Error::Path(structfs_core_store::PathError::InvalidPath {
                    message: msg,
                })
            }
            Error::LegacyStore(e) => structfs_core_store::Error::Other {
                message: e.to_string(),
            },
            Error::Serde(msg) => structfs_core_store::Error::Decode {
                format: structfs_core_store::Format::JSON,
                message: msg,
            },
        }
    }
}
