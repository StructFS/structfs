//! Error types for the Core layer.

use crate::format::Format;
use crate::path::{Path, PathError};

/// Errors at the Core layer.
///
/// These include semantic errors (invalid paths, codec failures) in addition
/// to the transport errors from the LL layer.
#[derive(Debug)]
pub enum Error {
    /// Path validation error.
    Path(PathError),

    /// Invalid path for an operation.
    InvalidPath { message: String },

    /// No route found for path (in overlay/mount stores).
    NoRoute { path: Path },

    /// Codec failed to decode bytes.
    Decode { format: Format, message: String },

    /// Codec failed to encode value.
    Encode { format: Format, message: String },

    /// Format not supported by codec.
    UnsupportedFormat(Format),

    /// Error from the LL layer.
    Ll(structfs_ll_store::LLError),

    /// Generic error with message.
    Other { message: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Path(e) => write!(f, "path error: {}", e),
            Error::InvalidPath { message } => write!(f, "invalid path: {}", message),
            Error::NoRoute { path } => write!(f, "no route for path: {}", path),
            Error::Decode { format, message } => {
                write!(f, "decode error ({}): {}", format, message)
            }
            Error::Encode { format, message } => {
                write!(f, "encode error ({}): {}", format, message)
            }
            Error::UnsupportedFormat(format) => {
                write!(f, "unsupported format: {}", format)
            }
            Error::Ll(e) => write!(f, "ll error: {}", e),
            Error::Other { message } => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Path(e) => Some(e),
            Error::Ll(e) => Some(e),
            _ => None,
        }
    }
}

impl From<PathError> for Error {
    fn from(e: PathError) -> Self {
        Error::Path(e)
    }
}

impl From<structfs_ll_store::LLError> for Error {
    fn from(e: structfs_ll_store::LLError) -> Self {
        Error::Ll(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as StdError;

    #[test]
    fn error_display() {
        let e = Error::NoRoute {
            path: Path::parse("foo/bar").unwrap(),
        };
        assert!(format!("{}", e).contains("foo/bar"));

        let e = Error::UnsupportedFormat(Format::PROTOBUF);
        assert!(format!("{}", e).contains("protobuf"));
    }

    #[test]
    fn path_error_display() {
        let e = Error::Path(PathError::InvalidComponent {
            component: "bad".to_string(),
            position: 1,
            message: "invalid".to_string(),
        });
        assert!(format!("{}", e).contains("path error"));
    }

    #[test]
    fn invalid_path_display() {
        let e = Error::InvalidPath {
            message: "bad path".to_string(),
        };
        assert!(format!("{}", e).contains("invalid path"));
        assert!(format!("{}", e).contains("bad path"));
    }

    #[test]
    fn decode_error_display() {
        let e = Error::Decode {
            format: Format::JSON,
            message: "unexpected token".to_string(),
        };
        let display = format!("{}", e);
        assert!(display.contains("decode error"));
        assert!(display.contains("json"));
        assert!(display.contains("unexpected token"));
    }

    #[test]
    fn encode_error_display() {
        let e = Error::Encode {
            format: Format::CBOR,
            message: "serialization failed".to_string(),
        };
        let display = format!("{}", e);
        assert!(display.contains("encode error"));
        assert!(display.contains("cbor"));
        assert!(display.contains("serialization failed"));
    }

    #[test]
    fn ll_error_display() {
        let ll_err = structfs_ll_store::LLError::NotSupported;
        let e = Error::Ll(ll_err);
        let display = format!("{}", e);
        assert!(display.contains("ll error"));
    }

    #[test]
    fn other_error_display() {
        let e = Error::Other {
            message: "something went wrong".to_string(),
        };
        assert_eq!(format!("{}", e), "something went wrong");
    }

    #[test]
    fn path_error_source() {
        let e = Error::Path(PathError::InvalidPath {
            message: "test".to_string(),
        });
        assert!(StdError::source(&e).is_some());
    }

    #[test]
    fn ll_error_source() {
        let ll_err = structfs_ll_store::LLError::NotSupported;
        let e = Error::Ll(ll_err);
        assert!(StdError::source(&e).is_some());
    }

    #[test]
    fn other_error_source_is_none() {
        let e = Error::Other {
            message: "test".to_string(),
        };
        assert!(StdError::source(&e).is_none());
    }

    #[test]
    fn path_error_conversion() {
        let path_err = PathError::InvalidPath {
            message: "test".to_string(),
        };
        let e: Error = path_err.into();
        assert!(matches!(e, Error::Path(_)));
    }

    #[test]
    fn ll_error_conversion() {
        let ll_err = structfs_ll_store::LLError::ResourceExhausted;
        let e: Error = ll_err.into();
        assert!(matches!(e, Error::Ll(_)));
    }
}
