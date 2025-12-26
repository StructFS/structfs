//! Error types for the Core layer.

use crate::format::Format;
use crate::path::{Path, PathError};

/// Whether a codec error occurred during encoding or decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecOperation {
    Encode,
    Decode,
}

impl std::fmt::Display for CodecOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecOperation::Encode => write!(f, "encode"),
            CodecOperation::Decode => write!(f, "decode"),
        }
    }
}

/// Errors at the Core layer.
///
/// These include semantic errors (invalid paths, codec failures) in addition
/// to the transport errors from the LL layer.
#[derive(Debug)]
pub enum Error {
    /// Path validation error.
    Path(PathError),

    /// No route found for path (in overlay/mount stores).
    NoRoute { path: Path },

    /// Codec error during encode/decode.
    Codec {
        operation: CodecOperation,
        format: Format,
        message: String,
    },

    /// Format not supported by codec.
    UnsupportedFormat(Format),

    /// Error from the LL layer.
    Ll(structfs_ll_store::LLError),

    /// I/O error (filesystem, network).
    Io(std::io::Error),

    /// Store-specific error with context.
    Store {
        store: &'static str,
        operation: &'static str,
        message: String,
    },
}

impl Error {
    /// Create a store-specific error.
    pub fn store(store: &'static str, operation: &'static str, message: impl Into<String>) -> Self {
        Error::Store {
            store,
            operation,
            message: message.into(),
        }
    }

    /// Create a codec decode error.
    pub fn decode(format: Format, message: impl Into<String>) -> Self {
        Error::Codec {
            operation: CodecOperation::Decode,
            format,
            message: message.into(),
        }
    }

    /// Create a codec encode error.
    pub fn encode(format: Format, message: impl Into<String>) -> Self {
        Error::Codec {
            operation: CodecOperation::Encode,
            format,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Path(e) => write!(f, "path error: {}", e),
            Error::NoRoute { path } => write!(f, "no route to {}", path),
            Error::Codec {
                operation,
                format,
                message,
            } => {
                write!(f, "{} failed for format {}: {}", operation, format, message)
            }
            Error::UnsupportedFormat(format) => {
                write!(f, "unsupported format: {}", format)
            }
            Error::Ll(e) => write!(f, "low-level error: {}", e),
            Error::Io(e) => write!(f, "I/O error: {}", e),
            Error::Store {
                store,
                operation,
                message,
            } => write!(f, "{}::{}: {}", store, operation, message),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Path(e) => Some(e),
            Error::Ll(e) => Some(e),
            Error::Io(e) => Some(e),
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

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
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
        assert_eq!(e.to_string(), "no route to foo/bar");

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
    fn codec_decode_error_display() {
        let e = Error::decode(Format::JSON, "unexpected token");
        let display = format!("{}", e);
        assert!(display.contains("decode"));
        assert!(display.contains("json"));
        assert!(display.contains("unexpected token"));
    }

    #[test]
    fn codec_encode_error_display() {
        let e = Error::encode(Format::CBOR, "serialization failed");
        let display = format!("{}", e);
        assert!(display.contains("encode"));
        assert!(display.contains("cbor"));
        assert!(display.contains("serialization failed"));
    }

    #[test]
    fn ll_error_display() {
        let ll_err = structfs_ll_store::LLError::NotSupported;
        let e = Error::Ll(ll_err);
        let display = format!("{}", e);
        assert!(display.contains("low-level error"));
    }

    #[test]
    fn store_error_display() {
        let e = Error::store("http_broker", "read", "Request 42 not found");
        assert_eq!(e.to_string(), "http_broker::read: Request 42 not found");
    }

    #[test]
    fn io_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = Error::Io(io_err);
        let display = format!("{}", e);
        assert!(display.contains("I/O error"));
        assert!(display.contains("file not found"));
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
    fn io_error_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = Error::Io(io_err);
        assert!(StdError::source(&e).is_some());
    }

    #[test]
    fn store_error_source_is_none() {
        let e = Error::store("test", "op", "message");
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

    #[test]
    fn io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e: Error = io_err.into();
        assert!(matches!(e, Error::Io(_)));
    }

    #[test]
    fn codec_operation_display() {
        assert_eq!(CodecOperation::Encode.to_string(), "encode");
        assert_eq!(CodecOperation::Decode.to_string(), "decode");
    }

    #[test]
    fn codec_error_with_operation() {
        let e = Error::Codec {
            operation: CodecOperation::Decode,
            format: Format::JSON,
            message: "test".to_string(),
        };
        assert!(matches!(
            e,
            Error::Codec {
                operation: CodecOperation::Decode,
                ..
            }
        ));
    }
}
