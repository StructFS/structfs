//! The crate's error type and its mapping onto core-store's typed errors.
//!
//! # Error mapping
//!
//! [`From<Error> for CoreError`] is the single seam where HTTP failures
//! become store failures. It is deliberately *typed*: callers above a store
//! match on [`structfs_core_store::Error`] variants instead of grepping
//! message strings.
//!
//! | This crate | `structfs_core_store::Error` |
//! |---|---|
//! | `Store(core)` | the nested error, unchanged |
//! | `Http(e)` where `e.is_timeout()` | `DeadlineExceeded` |
//! | `Http(e)` where `e.is_builder()` (malformed URL, unsupported scheme) | `InvalidArgument` |
//! | `Http(e)` where `e.is_connect()` and the source chain holds an `io::Error` of kind `ConnectionRefused` / `ConnectionReset` | `Overloaded` (transient; retry may succeed) |
//! | `Http(e)` any other connect failure (DNS, TLS handshake, certificate) | `Store { store: "http" }`, message kept |
//! | `Http(e)` carrying 401 / 403 | `PermissionDenied` |
//! | `Http(e)` carrying 404 | `Store { store: "http" }` (no store path is known) |
//! | `Status { path: Some(..) }` carrying 404 | `NotFound` for that path |
//! | `Http(e)` carrying 408 / 504 | `DeadlineExceeded` |
//! | `Http(e)` carrying 429 / 503 | `Overloaded` |
//! | `Http(e)` otherwise | `Store { store: "http" }` |
//! | `Status { .. }` | same status mapping as above |
//! | `UrlParse`, `InvalidUrl`, `InvalidMethod`, `InvalidHeader*` | `InvalidArgument` |
//! | `Json(e)` | `Codec` (JSON decode) |
//! | `ClientBuild`, `Runtime`, `Other` | `Store { store: "http" }` |
//!
//! A response status only becomes a typed error when a store decides the
//! status *is* the failure; `HttpClientStore::read` maps 404 to `Ok(None)`
//! before it reaches here, per the [`structfs_core_store::Reader`] contract.

use structfs_core_store::{Error as CoreError, Format};

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[cfg(any(feature = "blocking", feature = "streaming"))]
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Invalid URL: {message}")]
    InvalidUrl { message: String },

    /// The HTTP client could not be constructed (TLS backend, resolver,
    /// proxy configuration). Distinct from [`Error::InvalidUrl`]: nothing
    /// about the caller's URL is wrong.
    #[error("HTTP client construction failed: {message}")]
    ClientBuild { message: String },

    #[error("Broker runtime error: {message}")]
    Runtime { message: String },

    #[error("Invalid HTTP method: {method}")]
    InvalidMethod { method: String },

    #[error("Invalid header name: {0}")]
    InvalidHeaderName(#[from] http::header::InvalidHeaderName),

    #[error("Invalid header value: {0}")]
    InvalidHeaderValue(#[from] http::header::InvalidHeaderValue),

    /// A response whose status a store treats as the failure itself.
    ///
    /// `path` is the *store* path the operation addressed, when the store
    /// knows it; it is what a `404` needs to become a meaningful
    /// [`structfs_core_store::Error::NotFound`].
    #[error("HTTP {status} {status_text}: {message}")]
    Status {
        status: u16,
        status_text: String,
        message: String,
        path: Option<structfs_core_store::Path>,
    },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Store error: {0}")]
    Store(#[from] CoreError),

    #[error("{message}")]
    Other { message: String },
}

impl Error {
    /// A [`Error::Status`] built from a response's status line and body.
    pub fn status(status: u16, status_text: impl Into<String>, message: impl Into<String>) -> Self {
        Error::Status {
            status,
            status_text: status_text.into(),
            message: message.into(),
            path: None,
        }
    }

    /// As [`Error::status`], but recording the store path the operation
    /// addressed so a `404` can become a typed `NotFound`.
    pub fn status_at(
        path: structfs_core_store::Path,
        status: u16,
        status_text: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Error::Status {
            status,
            status_text: status_text.into(),
            message: message.into(),
            path: Some(path),
        }
    }
}

/// Map an HTTP status a store has decided is a failure onto a typed core
/// error, or `None` when no typed variant fits better than `Store`.
///
/// `404` only becomes `NotFound` when the store path is known — an HTTP URL
/// is not a store path, and a `NotFound` carrying an empty path is less
/// useful than the full status message.
fn typed_status(
    status: u16,
    message: String,
    path: Option<structfs_core_store::Path>,
) -> Option<CoreError> {
    match status {
        401 | 403 => Some(CoreError::permission_denied(message)),
        404 => path.map(CoreError::not_found),
        408 | 504 => Some(CoreError::deadline_exceeded(message)),
        429 | 503 => Some(CoreError::overloaded(message)),
        _ => None,
    }
}

/// Whether an error's source chain bottoms out in an `io::Error` that a
/// retry can plausibly fix: the peer refused or reset the connection.
///
/// reqwest's `is_connect()` is much broader — it also covers DNS NXDOMAIN,
/// TLS handshake and certificate failures — and those must *not* become
/// `Overloaded`, which callers (featherweight among them) treat as
/// retryable. Anything not recognised here stays `Store` with its message.
#[cfg(any(feature = "blocking", feature = "streaming"))]
pub(crate) fn is_transient_io(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = current {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset
            ) {
                return true;
            }
        }
        current = e.source();
    }
    false
}

impl From<Error> for CoreError {
    fn from(error: Error) -> Self {
        let message = error.to_string();
        match error {
            // Nested typed errors survive the hop; flattening them into
            // `Store` is what made timeouts and permission failures
            // indistinguishable from any other string.
            Error::Store(core) => core,

            #[cfg(any(feature = "blocking", feature = "streaming"))]
            Error::Http(ref e) => {
                if e.is_timeout() {
                    CoreError::deadline_exceeded(message)
                } else if e.is_builder() {
                    // The request could not even be built: a malformed URL
                    // or an unsupported scheme. Retrying cannot help.
                    CoreError::invalid_argument(message)
                } else if e.is_connect() && is_transient_io(e) {
                    CoreError::overloaded(message)
                } else if let Some(typed) = e
                    .status()
                    .and_then(|status| typed_status(status.as_u16(), message.clone(), None))
                {
                    typed
                } else {
                    CoreError::store("http", "request", message)
                }
            }

            Error::Status { status, path, .. } => typed_status(status, message.clone(), path)
                .unwrap_or_else(|| CoreError::store("http", "request", message)),

            Error::UrlParse(_)
            | Error::InvalidUrl { .. }
            | Error::InvalidMethod { .. }
            | Error::InvalidHeaderName(_)
            | Error::InvalidHeaderValue(_) => CoreError::invalid_argument(message),

            Error::Json(_) => CoreError::decode(Format::JSON, message),

            Error::ClientBuild { .. } | Error::Runtime { .. } | Error::Other { .. } => {
                CoreError::store("http", "request", message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_url_error_display() {
        let e = Error::InvalidUrl {
            message: "missing scheme".to_string(),
        };
        assert!(e.to_string().contains("missing scheme"));
    }

    #[test]
    fn invalid_method_error_display() {
        let e = Error::InvalidMethod {
            method: "FOOBAR".to_string(),
        };
        assert!(e.to_string().contains("FOOBAR"));
    }

    #[test]
    fn json_error_conversion() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let e = Error::from(json_err);
        assert!(e.to_string().contains("JSON error"));
        assert!(matches!(CoreError::from(e), CoreError::Codec { .. }));
    }

    #[test]
    fn store_error_conversion() {
        let store_err = CoreError::store("test", "op", "test error");
        let e = Error::from(store_err);
        assert!(e.to_string().contains("Store error"));
    }

    #[test]
    fn nested_typed_core_errors_survive_the_hop() {
        for original in [
            CoreError::permission_denied("no"),
            CoreError::deadline_exceeded("slow"),
            CoreError::invalid_argument("bad"),
            CoreError::conflict("clash"),
            CoreError::overloaded("busy"),
            CoreError::resource_limit("big"),
            CoreError::Cancelled {
                message: "gone".into(),
            },
            CoreError::not_found(structfs_core_store::path!("a/b")),
        ] {
            let expected = std::mem::discriminant(&original);
            let round_tripped: CoreError = Error::Store(original).into();
            assert_eq!(std::mem::discriminant(&round_tripped), expected);
        }
    }

    #[test]
    fn argument_errors_map_to_invalid_argument() {
        for e in [
            Error::InvalidUrl {
                message: "bad url".into(),
            },
            Error::InvalidMethod {
                method: "PURGE".into(),
            },
            Error::from(url::Url::parse("not a url").unwrap_err()),
        ] {
            let core: CoreError = e.into();
            assert!(
                matches!(core, CoreError::InvalidArgument { .. }),
                "{core:?}"
            );
        }
    }

    #[test]
    fn statuses_map_to_typed_variants() {
        type Check = fn(&CoreError) -> bool;
        let cases: [(u16, Check); 7] = [
            (401, |e| matches!(e, CoreError::PermissionDenied { .. })),
            (403, |e| matches!(e, CoreError::PermissionDenied { .. })),
            (404, |e| matches!(e, CoreError::Store { .. })),
            (408, |e| matches!(e, CoreError::DeadlineExceeded { .. })),
            (429, |e| matches!(e, CoreError::Overloaded { .. })),
            (503, |e| matches!(e, CoreError::Overloaded { .. })),
            (500, |e| matches!(e, CoreError::Store { .. })),
        ];
        for (status, check) in cases {
            let core: CoreError = Error::status(status, "reason", "body").into();
            assert!(check(&core), "status {status} mapped to {core:?}");
        }
    }

    #[test]
    fn a_404_with_a_known_store_path_is_not_found() {
        let core: CoreError = Error::status_at(
            structfs_core_store::path!("users/1"),
            404,
            "Not Found",
            "gone",
        )
        .into();
        match core {
            CoreError::NotFound { path } => assert_eq!(path.to_string(), "users/1"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn client_build_failure_is_not_an_invalid_url() {
        let core: CoreError = Error::ClientBuild {
            message: "no TLS backend".into(),
        }
        .into();
        assert!(matches!(core, CoreError::Store { store: "http", .. }));
        assert!(!core.to_string().contains("Invalid URL"));
    }

    /// A two-level error chain, standing in for reqwest -> hyper -> io.
    #[cfg(any(feature = "blocking", feature = "streaming"))]
    #[derive(Debug, thiserror::Error)]
    #[error("connect failed")]
    struct Wrapper(#[source] std::io::Error);

    #[cfg(any(feature = "blocking", feature = "streaming"))]
    #[test]
    fn only_refused_or_reset_connections_count_as_transient() {
        use std::io::{Error as IoError, ErrorKind};

        for kind in [ErrorKind::ConnectionRefused, ErrorKind::ConnectionReset] {
            let chained = Wrapper(IoError::new(kind, "peer"));
            assert!(is_transient_io(&chained), "{kind:?}");
            assert!(is_transient_io(&IoError::new(kind, "direct")), "{kind:?}");
        }

        // DNS failures, TLS/certificate failures and everything else are
        // not transient: they must not become retryable `Overloaded`.
        for kind in [
            ErrorKind::NotFound,
            ErrorKind::InvalidData,
            ErrorKind::PermissionDenied,
            ErrorKind::Other,
        ] {
            assert!(
                !is_transient_io(&Wrapper(IoError::new(kind, "x"))),
                "{kind:?}"
            );
        }
        let no_io = CoreError::invalid_argument("not io");
        assert!(!is_transient_io(&no_io));
    }

    #[cfg(feature = "blocking")]
    #[test]
    fn reqwest_builder_errors_are_invalid_arguments() {
        let client = reqwest::blocking::Client::new();
        let error = client.get("ftp://[bad").build().unwrap_err();
        assert!(error.is_builder());
        let core: CoreError = Error::Http(error).into();
        assert!(
            matches!(core, CoreError::InvalidArgument { .. }),
            "{core:?}"
        );
    }

    #[test]
    fn url_parse_error_conversion() {
        let url_err = url::Url::parse("not a url").unwrap_err();
        let e = Error::from(url_err);
        assert!(e.to_string().contains("URL parse error"));
    }
}
