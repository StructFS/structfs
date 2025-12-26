//! Error types for the LL layer.
//!
//! Errors at this level are transport-focused. No semantic errors like
//! "invalid path format" or "type mismatch" - those belong in higher layers.

use bytes::Bytes;

/// Errors at the LL (low-level) layer.
///
/// These are transport and system-level errors only. Semantic errors
/// (invalid paths, type mismatches, codec failures) belong in higher layers.
#[derive(Debug)]
pub enum LLError {
    /// Generic I/O or transport failure.
    ///
    /// Use this for network errors, file I/O errors, IPC failures, etc.
    Transport(Box<dyn std::error::Error + Send + Sync>),

    /// The operation is not supported by this store.
    ///
    /// For example, writing to a read-only store.
    NotSupported,

    /// Resource limit exceeded.
    ///
    /// Memory exhaustion, too many open handles, etc.
    ResourceExhausted,

    /// Protocol-specific error with a numeric code.
    ///
    /// The code and detail are opaque to the LL layer. Higher layers
    /// or the transport protocol define their meaning.
    Protocol {
        /// Protocol-specific error code.
        code: u32,
        /// Optional detail bytes (error message, structured error, etc.)
        detail: Bytes,
    },
}

impl std::fmt::Display for LLError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LLError::Transport(e) => write!(f, "transport error: {}", e),
            LLError::NotSupported => write!(f, "operation not supported"),
            LLError::ResourceExhausted => write!(f, "resource exhausted"),
            LLError::Protocol { code, detail } => {
                if detail.is_empty() {
                    write!(f, "protocol error: code {}", code)
                } else {
                    // Try to display detail as UTF-8, fall back to hex
                    match std::str::from_utf8(detail) {
                        Ok(s) => write!(f, "protocol error: code {} - {}", code, s),
                        Err(_) => write!(f, "protocol error: code {} - {:?}", code, detail),
                    }
                }
            }
        }
    }
}

impl std::error::Error for LLError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LLError::Transport(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<std::io::Error> for LLError {
    fn from(e: std::io::Error) -> Self {
        LLError::Transport(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as StdError;

    #[test]
    fn error_display_works() {
        let e = LLError::NotSupported;
        assert_eq!(format!("{}", e), "operation not supported");

        let e = LLError::Protocol {
            code: 42,
            detail: Bytes::from_static(b"something went wrong"),
        };
        assert!(format!("{}", e).contains("42"));
        assert!(format!("{}", e).contains("something went wrong"));
    }

    #[test]
    fn resource_exhausted_display() {
        let e = LLError::ResourceExhausted;
        assert_eq!(format!("{}", e), "resource exhausted");
    }

    #[test]
    fn protocol_empty_detail_display() {
        let e = LLError::Protocol {
            code: 100,
            detail: Bytes::new(),
        };
        assert_eq!(format!("{}", e), "protocol error: code 100");
    }

    #[test]
    fn protocol_non_utf8_detail_display() {
        let e = LLError::Protocol {
            code: 200,
            detail: Bytes::from_static(&[0xFF, 0xFE, 0x00]),
        };
        let display = format!("{}", e);
        assert!(display.contains("200"));
        // Should fall back to debug format
        assert!(display.contains("protocol error"));
    }

    #[test]
    fn transport_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = LLError::Transport(Box::new(io_err));
        let display = format!("{}", e);
        assert!(display.contains("transport error"));
        assert!(display.contains("file not found"));
    }

    #[test]
    fn transport_error_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let e = LLError::Transport(Box::new(io_err));
        assert!(StdError::source(&e).is_some());
    }

    #[test]
    fn non_transport_error_source_is_none() {
        let e = LLError::NotSupported;
        assert!(StdError::source(&e).is_none());

        let e = LLError::ResourceExhausted;
        assert!(StdError::source(&e).is_none());

        let e = LLError::Protocol {
            code: 1,
            detail: Bytes::new(),
        };
        assert!(StdError::source(&e).is_none());
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let ll_err: LLError = io_err.into();
        assert!(matches!(ll_err, LLError::Transport(_)));
    }
}
