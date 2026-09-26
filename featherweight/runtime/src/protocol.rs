//! Server-protocol envelopes
//! ([spec 07](https://github.com/StructFS/structfs/blob/main/isotope/spec/07-server-protocol.md)).
//!
//! Requests flow runtime -> block as `{op, path, data, respond_to}`;
//! responses flow block -> runtime as `{result, value?, path?, error?}`.
//! This module is the single place both shapes are encoded and decoded —
//! blocks use it to serve, the runtime uses it to call.

use std::collections::BTreeMap;

use structfs_core_store::{CodecDiagnostic, CodecErrorKind, Error, Path, PathError, Value};

/// A request as decoded by a serving block.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RequestEnvelope {
    /// `"read"` or `"write"`.
    pub op: String,
    /// Path relative to the block's store root.
    pub path: Path,
    /// Data for writes; `Value::Null` for reads.
    pub data: Value,
    /// Where to write the response.
    pub respond_to: Path,
}

impl RequestEnvelope {
    /// Decode a request envelope from a Value.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = match value {
            Value::Map(map) => map,
            _ => return Err(Error::store("protocol", "request", "not a map")),
        };
        let op = match map.get("op") {
            Some(Value::String(op)) => op.clone(),
            _ => return Err(Error::store("protocol", "request", "missing op")),
        };
        let path = match map.get("path") {
            Some(Value::String(path)) => Path::parse(path)?,
            _ => return Err(Error::store("protocol", "request", "missing path")),
        };
        let respond_to = match map.get("respond_to") {
            Some(Value::String(path)) => Path::parse(path)?,
            _ => return Err(Error::store("protocol", "request", "missing respond_to")),
        };
        let data = map.get("data").cloned().unwrap_or(Value::Null);
        Ok(Self {
            op,
            path,
            data,
            respond_to,
        })
    }
}

/// One decoded mailbox event, as seen by a serving block
/// ([spec 09](https://github.com/StructFS/structfs/blob/main/isotope/spec/09-posix-closure.md)).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EventEnvelope {
    /// Shutdown was requested: the mailbox read unblocked with Null.
    Shutdown,
    /// A server-protocol request to serve.
    Request(RequestEnvelope),
    /// A runtime signal (fire-and-forget).
    Signal { name: String, data: Value },
    /// A timer the block registered has fired.
    Timer { tag: Value },
    /// An event with an op this decoder doesn't know; per spec, ignore it.
    Unknown(Value),
}

impl EventEnvelope {
    /// Decode a mailbox event from the value a `iso/server/requests` read
    /// returned.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let map = match value {
            Value::Null => return Ok(EventEnvelope::Shutdown),
            Value::Map(map) => map,
            _ => return Err(Error::store("protocol", "event", "not a map")),
        };
        match map.get("op") {
            Some(Value::String(op)) if op == "read" || op == "write" => {
                Ok(EventEnvelope::Request(RequestEnvelope::from_value(value)?))
            }
            Some(Value::String(op)) if op == "signal" => Ok(EventEnvelope::Signal {
                name: match map.get("signal") {
                    Some(Value::String(name)) => name.clone(),
                    _ => return Err(Error::store("protocol", "event", "signal missing name")),
                },
                data: map.get("data").cloned().unwrap_or(Value::Null),
            }),
            Some(Value::String(op)) if op == "timer" => Ok(EventEnvelope::Timer {
                tag: map.get("tag").cloned().unwrap_or(Value::Null),
            }),
            _ => Ok(EventEnvelope::Unknown(value.clone())),
        }
    }
}

/// Build a successful present read response: `{result: "ok", present: true, value}`.
pub fn ok_value(value: Value) -> Value {
    let mut map = BTreeMap::new();
    map.insert("result".to_string(), Value::from("ok"));
    map.insert("value".to_string(), value);
    map.insert("present".to_string(), Value::Bool(true));
    Value::Map(map)
}

/// Build an absent read response without conflating absence with Null.
pub fn ok_absent() -> Value {
    Value::Map(BTreeMap::from([
        ("result".into(), Value::from("ok")),
        ("present".into(), Value::Bool(false)),
    ]))
}

/// Build a successful write response: `{result: "ok", path}`.
pub fn ok_path(path: &Path) -> Value {
    let mut map = BTreeMap::new();
    map.insert("result".to_string(), Value::from("ok"));
    map.insert("path".to_string(), Value::String(path.to_string()));
    Value::Map(map)
}

/// The typed error taxonomy, shared by every boundary the runtime owns.
///
/// This is the single source of truth for how a core [`Error`] crosses a
/// boundary: its spec 11 status code ([`ErrorKind::status`]), its spec 07
/// server-protocol error type ([`ErrorKind::wire`]), and the label the
/// transcript and session log record ([`ErrorKind::label`]). The core-wasm
/// binding, the server protocol, transcripts, and the session log all
/// consume it, so the tables cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// An operation required a path that does not exist.
    NotFound,
    /// No store is mounted for the path.
    NoRoute,
    /// The caller may not perform the operation (including unwired paths).
    PermissionDenied,
    /// The request clashes with existing or concurrent state.
    Conflict,
    /// The request is malformed independent of store state.
    InvalidArgument,
    /// The store is temporarily unable to accept the operation.
    Overloaded,
    /// The operation did not complete within its deadline.
    DeadlineExceeded,
    /// A size, depth, count, or quota limit was exceeded.
    ResourceLimit,
    /// The operation was cancelled (an interrupted parked read).
    Cancelled,
    /// A path failed validation.
    InvalidPath,
    /// A codec failure other than a resource limit.
    Codec,
    /// A codec resource-limit failure.
    CodecResourceLimit,
    /// Anything else: store-specific, I/O, and low-level transport errors.
    Other,
}

/// Spec 11 status codes.
pub mod status {
    /// Success.
    pub const OK: i32 = 0;
    /// Absent (reads only).
    pub const ABSENT: i32 = 1;
    pub const NOT_FOUND: i32 = -1;
    pub const PERMISSION_DENIED: i32 = -2;
    pub const CONFLICT: i32 = -3;
    pub const OVERLOADED: i32 = -4;
    pub const DEADLINE_EXCEEDED: i32 = -5;
    pub const CANCELLED: i32 = -6;
    pub const INVALID_PATH: i32 = -7;
    pub const RESOURCE_LIMIT: i32 = -8;
    pub const OTHER: i32 = -9;
    pub const INVALID_ARGUMENT: i32 = -10;
}

impl ErrorKind {
    /// Every kind, for exhaustive round-trip checks.
    pub const ALL: [ErrorKind; 13] = [
        ErrorKind::NotFound,
        ErrorKind::NoRoute,
        ErrorKind::PermissionDenied,
        ErrorKind::Conflict,
        ErrorKind::InvalidArgument,
        ErrorKind::Overloaded,
        ErrorKind::DeadlineExceeded,
        ErrorKind::ResourceLimit,
        ErrorKind::Cancelled,
        ErrorKind::InvalidPath,
        ErrorKind::Codec,
        ErrorKind::CodecResourceLimit,
        ErrorKind::Other,
    ];

    /// Classify a core error.
    pub fn of(error: &Error) -> Self {
        match error {
            Error::NotFound { .. } => ErrorKind::NotFound,
            Error::NoRoute { .. } => ErrorKind::NoRoute,
            Error::PermissionDenied { .. } => ErrorKind::PermissionDenied,
            Error::Conflict { .. } => ErrorKind::Conflict,
            Error::InvalidArgument { .. } => ErrorKind::InvalidArgument,
            Error::Overloaded { .. } => ErrorKind::Overloaded,
            Error::DeadlineExceeded { .. } => ErrorKind::DeadlineExceeded,
            Error::ResourceLimit { .. } => ErrorKind::ResourceLimit,
            Error::Cancelled { .. } => ErrorKind::Cancelled,
            Error::Path(_) => ErrorKind::InvalidPath,
            Error::Codec {
                kind: CodecErrorKind::ResourceLimit,
                ..
            } => ErrorKind::CodecResourceLimit,
            Error::Codec { .. } | Error::UnsupportedFormat(_) => ErrorKind::Codec,
            _ => ErrorKind::Other,
        }
    }

    /// The spec 11 status code.
    pub const fn status(self) -> i32 {
        match self {
            ErrorKind::NotFound | ErrorKind::NoRoute => status::NOT_FOUND,
            ErrorKind::PermissionDenied => status::PERMISSION_DENIED,
            ErrorKind::Conflict => status::CONFLICT,
            ErrorKind::Overloaded => status::OVERLOADED,
            ErrorKind::DeadlineExceeded => status::DEADLINE_EXCEEDED,
            ErrorKind::Cancelled => status::CANCELLED,
            ErrorKind::InvalidPath => status::INVALID_PATH,
            ErrorKind::ResourceLimit | ErrorKind::CodecResourceLimit => status::RESOURCE_LIMIT,
            ErrorKind::InvalidArgument => status::INVALID_ARGUMENT,
            ErrorKind::Codec | ErrorKind::Other => status::OTHER,
        }
    }

    /// The spec 07 server-protocol error `type`.
    pub const fn wire(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "not_found",
            ErrorKind::NoRoute => "no_route",
            ErrorKind::PermissionDenied => "forbidden",
            ErrorKind::Conflict => "conflict",
            ErrorKind::InvalidArgument => "invalid_argument",
            ErrorKind::Overloaded => "unavailable",
            ErrorKind::DeadlineExceeded => "timeout",
            ErrorKind::ResourceLimit | ErrorKind::CodecResourceLimit => "resource_limit",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::InvalidPath => "invalid_path",
            ErrorKind::Codec | ErrorKind::Other => "store_error",
        }
    }

    /// Parse a spec 07 error `type`, including the legacy aliases.
    pub fn from_wire(name: &str) -> Self {
        match name {
            "not_found" => ErrorKind::NotFound,
            "no_route" => ErrorKind::NoRoute,
            "forbidden" | "not_readable" | "not_writable" => ErrorKind::PermissionDenied,
            "conflict" => ErrorKind::Conflict,
            "invalid_argument" => ErrorKind::InvalidArgument,
            "unavailable" => ErrorKind::Overloaded,
            "timeout" => ErrorKind::DeadlineExceeded,
            "resource_limit" => ErrorKind::ResourceLimit,
            "cancelled" => ErrorKind::Cancelled,
            "invalid_path" => ErrorKind::InvalidPath,
            _ => ErrorKind::Other,
        }
    }

    /// The kind label transcripts and the session log record (spec 12).
    pub const fn label(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "not_found",
            ErrorKind::NoRoute => "no_route",
            ErrorKind::PermissionDenied => "permission_denied",
            ErrorKind::Conflict => "conflict",
            ErrorKind::InvalidArgument => "invalid_argument",
            ErrorKind::Overloaded => "overloaded",
            ErrorKind::DeadlineExceeded => "deadline_exceeded",
            ErrorKind::ResourceLimit => "resource_limit",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::InvalidPath => "invalid_path",
            ErrorKind::Codec | ErrorKind::CodecResourceLimit => "codec",
            ErrorKind::Other => "other",
        }
    }

    /// Parse a transcript/session label; unknown labels are [`ErrorKind::Other`].
    pub fn from_label(name: &str) -> Self {
        match name {
            "not_found" => ErrorKind::NotFound,
            "no_route" => ErrorKind::NoRoute,
            "permission_denied" => ErrorKind::PermissionDenied,
            "conflict" => ErrorKind::Conflict,
            "invalid_argument" => ErrorKind::InvalidArgument,
            "overloaded" => ErrorKind::Overloaded,
            "deadline_exceeded" => ErrorKind::DeadlineExceeded,
            "resource_limit" => ErrorKind::ResourceLimit,
            "cancelled" => ErrorKind::Cancelled,
            "invalid_path" => ErrorKind::InvalidPath,
            "codec" => ErrorKind::Codec,
            _ => ErrorKind::Other,
        }
    }

    /// Whether retrying the same request may succeed without changing it.
    /// Retrying is still not permission to repeat an effect.
    pub const fn retryable(self) -> bool {
        matches!(self, ErrorKind::Overloaded | ErrorKind::DeadlineExceeded)
    }

    /// The kind a spec 11 status denotes on its own, for hosts and guests
    /// that see only the status: `-1` is not found, `-8` a resource limit,
    /// `-9` other. `None` for success, absence, and unknown codes.
    pub const fn from_status(code: i32) -> Option<Self> {
        Some(match code {
            status::NOT_FOUND => ErrorKind::NotFound,
            status::PERMISSION_DENIED => ErrorKind::PermissionDenied,
            status::CONFLICT => ErrorKind::Conflict,
            status::OVERLOADED => ErrorKind::Overloaded,
            status::DEADLINE_EXCEEDED => ErrorKind::DeadlineExceeded,
            status::CANCELLED => ErrorKind::Cancelled,
            status::INVALID_PATH => ErrorKind::InvalidPath,
            status::RESOURCE_LIMIT => ErrorKind::ResourceLimit,
            status::OTHER => ErrorKind::Other,
            status::INVALID_ARGUMENT => ErrorKind::InvalidArgument,
            _ => return None,
        })
    }

    /// The whole taxonomy as JSON, one row per kind: `kind`, `status`,
    /// `wire`, `label`, `retryable`, and `canonical` (whether the kind is
    /// what its status denotes alone). Other implementations of the
    /// boundary — the TypeScript browser host — pin their tables to this
    /// rendering through the committed `error-kinds.json` fixture.
    pub fn table_json() -> String {
        let rows: Vec<String> = Self::ALL
            .iter()
            .map(|kind| {
                serde_json::json!({
                    "kind": format!("{kind:?}"),
                    "status": kind.status(),
                    "wire": kind.wire(),
                    "label": kind.label(),
                    "retryable": kind.retryable(),
                    "canonical": Self::from_status(kind.status()) == Some(*kind),
                })
                .to_string()
            })
            .collect();
        format!("[\n{}\n]\n", rows.join(",\n"))
    }
}

/// A core error decomposed into the fields every boundary carries: its
/// kind, its own message (without the kind's display prefix), and the
/// structured detail that typed reconstruction needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ErrorParts {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    /// `NotFound` / `NoRoute`: the path.
    pub(crate) path: Option<Path>,
    /// `InvalidPath`: the offending component and its position.
    pub(crate) component: Option<(String, u64)>,
    /// Codec-family errors: the portable diagnostic.
    pub(crate) codec: Option<CodecDiagnostic>,
}

impl ErrorParts {
    pub(crate) fn of(error: &Error) -> Self {
        let mut parts = ErrorParts {
            kind: ErrorKind::of(error),
            message: String::new(),
            path: None,
            component: None,
            codec: error.codec_diagnostic(),
        };
        match error {
            Error::NotFound { path } | Error::NoRoute { path } => parts.path = Some(path.clone()),
            Error::PermissionDenied { message }
            | Error::Conflict { message }
            | Error::InvalidArgument { message }
            | Error::Overloaded { message }
            | Error::DeadlineExceeded { message }
            | Error::ResourceLimit { message }
            | Error::Cancelled { message } => parts.message = message.clone(),
            Error::Path(PathError::InvalidComponent {
                component,
                position,
                message,
            }) => {
                parts.message = message.clone();
                parts.component = Some((component.clone(), *position as u64));
            }
            other => parts.message = other.to_string(),
        }
        parts
    }

    /// Rebuild the typed error. Kinds without a typed home become
    /// `Error::Store { store, operation, .. }` carrying the message.
    pub(crate) fn into_error(self, store: &'static str, operation: &'static str) -> Error {
        if let Some(codec) = self.codec {
            if let Some(error) = codec.into_error(self.message.clone()) {
                return error;
            }
        }
        let message = self.message;
        match self.kind {
            ErrorKind::NotFound => match self.path {
                Some(path) => Error::not_found(path),
                None => Error::store(store, operation, format!("not found: {message}")),
            },
            ErrorKind::NoRoute => match self.path {
                Some(path) => Error::NoRoute { path },
                None => Error::store(store, operation, format!("no route: {message}")),
            },
            ErrorKind::PermissionDenied => Error::permission_denied(message),
            ErrorKind::Conflict => Error::conflict(message),
            ErrorKind::InvalidArgument => Error::invalid_argument(message),
            ErrorKind::Overloaded => Error::overloaded(message),
            ErrorKind::DeadlineExceeded => Error::deadline_exceeded(message),
            ErrorKind::ResourceLimit | ErrorKind::CodecResourceLimit => {
                Error::resource_limit(message)
            }
            ErrorKind::Cancelled => Error::cancelled(message),
            ErrorKind::InvalidPath => {
                let (component, position) = self.component.unwrap_or_default();
                Error::Path(PathError::InvalidComponent {
                    component,
                    position: position as usize,
                    message,
                })
            }
            ErrorKind::Codec | ErrorKind::Other => Error::store(store, operation, message),
        }
    }
}

/// Build an error response with the spec's error taxonomy.
pub fn err_response(error_type: &str, message: &str, retryable: bool) -> Value {
    let mut error = BTreeMap::new();
    error.insert("type".to_string(), Value::from(error_type));
    error.insert("message".to_string(), Value::from(message));
    error.insert("retryable".to_string(), Value::Bool(retryable));
    let mut map = BTreeMap::new();
    map.insert("result".to_string(), Value::from("error"));
    map.insert("error".to_string(), Value::Map(error));
    Value::Map(map)
}

/// Encode a store error as a spec error response.
///
/// Errors are expressed in store-level terms only — the caller must not be
/// able to tell what implementation is behind the path: an overload or a
/// cancellation carries a fixed message, never the provider's own text.
/// The optional `path` (not found / no route), `component`/`position`
/// (invalid path), and `codec` fields let a reader rebuild the typed error.
pub fn error_to_response(error: &Error) -> Value {
    let parts = ErrorParts::of(error);
    let message = match parts.kind {
        ErrorKind::Overloaded => "store temporarily unavailable".to_string(),
        ErrorKind::Cancelled => "operation cancelled".to_string(),
        ErrorKind::NotFound | ErrorKind::NoRoute => error.to_string(),
        _ => parts.message.clone(),
    };
    let mut response = err_response(parts.kind.wire(), &message, parts.kind.retryable());
    if let Value::Map(root) = &mut response {
        if let Some(Value::Map(fields)) = root.get_mut("error") {
            if let Some(path) = &parts.path {
                fields.insert("path".into(), Value::String(path.to_string()));
            }
            if let Some((component, position)) = parts.component {
                fields.insert("component".into(), Value::String(component));
                fields.insert("position".into(), Value::Unsigned(position));
            }
            if let Some(detail) = &parts.codec {
                if let Ok(value) = structfs_serde_store::to_value(detail) {
                    fields.insert("codec".into(), value);
                }
            }
        }
    }
    response
}

fn decode_error(map: &BTreeMap<String, Value>) -> Error {
    let Some(Value::Map(fields)) = map.get("error") else {
        return Error::store(
            "server_protocol",
            "call",
            "malformed error response".to_string(),
        );
    };
    let kind = match fields.get("type") {
        Some(Value::String(t)) => ErrorKind::from_wire(t),
        _ => ErrorKind::Other,
    };
    let message = match fields.get("message") {
        Some(Value::String(m)) => m.clone(),
        _ => "unknown error".to_string(),
    };
    let path = match fields.get("path") {
        Some(Value::String(p)) => Path::parse(p).ok(),
        _ => None,
    };
    let component = match (fields.get("component"), fields.get("position")) {
        (Some(Value::String(component)), Some(Value::Unsigned(position))) => {
            Some((component.clone(), *position))
        }
        (Some(Value::String(component)), Some(Value::Integer(position))) => {
            Some((component.clone(), (*position).max(0) as u64))
        }
        _ => None,
    };
    let codec = fields.get("codec").and_then(|detail| {
        structfs_serde_store::from_value::<CodecDiagnostic>(detail.clone()).ok()
    });
    // A not-found message on the wire is the whole display text; the path
    // field alone rebuilds the typed error.
    ErrorParts {
        kind,
        message,
        path,
        component,
        codec,
    }
    .into_error("server_protocol", "call")
}

/// Decode canonical explicit presence. Unmarked responses are malformed.
pub fn decode_read_response(response: Value) -> Result<Option<Value>, Error> {
    let map = match response {
        Value::Map(map) => map,
        _ => {
            return Err(Error::store(
                "server_protocol",
                "read",
                "response is not a map",
            ))
        }
    };
    match map.get("result") {
        Some(Value::String(result)) if result == "ok" => match map.get("present") {
            Some(Value::Bool(true)) => map
                .get("value")
                .cloned()
                .map(Some)
                .ok_or_else(|| Error::conflict("present response missing value")),
            Some(Value::Bool(false)) if !map.contains_key("value") => Ok(None),
            Some(_) => Err(Error::conflict("invalid response presence")),
            None => Err(Error::conflict("response missing presence")),
        },
        _ => Err(decode_error(&map)),
    }
}

/// Decode a response to a write, yielding the result path.
pub fn decode_write_response(response: Value) -> Result<Path, Error> {
    let map = match response {
        Value::Map(map) => map,
        _ => {
            return Err(Error::store(
                "server_protocol",
                "write",
                "response is not a map",
            ))
        }
    };
    match map.get("result") {
        Some(Value::String(result)) if result == "ok" => match map.get("path") {
            Some(Value::String(path)) => Ok(Path::parse(path)?),
            _ => Err(Error::store(
                "server_protocol",
                "write",
                "ok response missing path",
            )),
        },
        _ => Err(decode_error(&map)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::block::ServerRequest;
    use structfs_core_store::path;

    #[test]
    fn request_round_trip() {
        let request = ServerRequest {
            op: "write",
            path: path!("users/1"),
            data: Value::from(5i64),
            token: 3,
        };
        let envelope = RequestEnvelope::from_value(&request.to_value()).unwrap();
        assert_eq!(envelope.op, "write");
        assert_eq!(envelope.path, path!("users/1"));
        assert_eq!(envelope.data, Value::Integer(5));
        assert_eq!(envelope.respond_to, path!("iso/server/responses/3"));
    }

    #[test]
    fn read_response_round_trip() {
        assert_eq!(
            decode_read_response(ok_value(Value::from("x"))).unwrap(),
            Some(Value::from("x"))
        );
        assert_eq!(
            decode_read_response(ok_value(Value::Null)).unwrap(),
            Some(Value::Null)
        );
        assert_eq!(decode_read_response(ok_absent()).unwrap(), None);
        let legacy = Value::Map(BTreeMap::from([
            ("result".into(), "ok".into()),
            ("value".into(), Value::Null),
        ]));
        assert!(decode_read_response(legacy).is_err());
        let invalid = Value::Map(BTreeMap::from([
            ("result".into(), "ok".into()),
            ("present".into(), true.into()),
        ]));
        assert!(decode_read_response(invalid).is_err());
    }

    #[test]
    fn write_response_round_trip() {
        assert_eq!(
            decode_write_response(ok_path(&path!("outstanding/1"))).unwrap(),
            path!("outstanding/1")
        );
    }

    /// One sample of every core `Error` variant.
    pub(crate) fn every_error() -> Vec<Error> {
        use structfs_core_store::{CodecOperation, Format};
        vec![
            Error::Path(PathError::InvalidComponent {
                component: "bad-name".into(),
                position: 1,
                message: "not an identifier".into(),
            }),
            Error::NoRoute {
                path: path!("unmounted/x"),
            },
            Error::Codec {
                kind: CodecErrorKind::TypeMismatch,
                operation: CodecOperation::Decode,
                format: Format::JSON,
                message: "expected map".into(),
            },
            Error::Codec {
                kind: CodecErrorKind::ResourceLimit,
                operation: CodecOperation::Decode,
                format: Format::CBOR,
                message: "too deep".into(),
            },
            Error::UnsupportedFormat(Format::OCTET_STREAM),
            Error::Ll(structfs_ll_store::LLError::NotSupported),
            Error::Io(std::io::Error::other("disk")),
            Error::store("http", "read", "boom"),
            Error::not_found(path!("users/nobody")),
            Error::permission_denied("unwired"),
            Error::conflict("stale"),
            Error::invalid_argument("zero limit"),
            Error::overloaded("busy"),
            Error::deadline_exceeded("30s"),
            Error::resource_limit("quota"),
            Error::cancelled("shutdown"),
        ]
    }

    #[test]
    fn every_error_variant_round_trips_through_the_wire() {
        let mut kinds = std::collections::BTreeSet::new();
        for error in every_error() {
            let decoded = decode_read_response(error_to_response(&error)).unwrap_err();
            let kind = ErrorKind::of(&error);
            kinds.insert(format!("{kind:?}"));
            assert_eq!(
                ErrorKind::of(&decoded),
                kind,
                "{error} came back as {decoded}"
            );
            // The status a core-wasm guest sees is preserved across the hop.
            assert_eq!(ErrorKind::of(&decoded).status(), kind.status(), "{error}");
            match (&error, &decoded) {
                (Error::NotFound { path: a }, Error::NotFound { path: b })
                | (Error::NoRoute { path: a }, Error::NoRoute { path: b }) => assert_eq!(a, b),
                (Error::Path(a), Error::Path(b)) => assert_eq!(a, b),
                (Error::Codec { .. } | Error::UnsupportedFormat(_), _) => {
                    assert_eq!(error.codec_diagnostic(), decoded.codec_diagnostic())
                }
                _ => {}
            }
        }
        // The samples cover every kind.
        assert_eq!(kinds.len(), ErrorKind::ALL.len());
    }

    #[test]
    fn kind_tables_are_mutually_consistent() {
        for kind in ErrorKind::ALL {
            // Labels and wire names parse back to a kind with the same
            // status (the codec pair shares one label and one wire name).
            assert!(
                ErrorKind::from_label(kind.label()).status() == kind.status()
                    || kind == ErrorKind::CodecResourceLimit,
                "{kind:?}"
            );
            assert_eq!(
                ErrorKind::from_wire(kind.wire()).status(),
                match kind {
                    ErrorKind::Codec => status::OTHER,
                    other => other.status(),
                },
                "{kind:?}"
            );
            assert!(kind.status() < 0);
        }
        assert_eq!(
            ErrorKind::InvalidArgument.status(),
            status::INVALID_ARGUMENT
        );
        assert_eq!(
            ErrorKind::from_wire("not_readable"),
            ErrorKind::PermissionDenied
        );
        for code in -10..=-1 {
            let kind = ErrorKind::from_status(code).expect("every spec 11 error status");
            assert_eq!(kind.status(), code);
        }
        assert_eq!(ErrorKind::from_status(status::OK), None);
        assert_eq!(ErrorKind::from_status(-11), None);
    }

    /// Where the taxonomy fixture lives: beside the native cross-host
    /// fixtures and, byte-identically, beside the browser host's tests.
    fn error_kind_fixtures() -> [std::path::PathBuf; 2] {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        [
            manifest.join("tests/fixtures/error-kinds.json"),
            manifest.join("../host/browser/test/fixtures/error-kinds.json"),
        ]
    }

    /// The browser host's error tables are tested against this fixture,
    /// so a change to the taxonomy fails here until the fixture — and
    /// with it the browser host — is updated.
    #[test]
    fn the_committed_error_kind_table_is_current() {
        let [native, browser] = error_kind_fixtures();
        // The browser copy lives outside this crate's package; an extracted
        // `.crate` checks only its own fixture.
        let checked = std::iter::once(native).chain(browser.exists().then_some(browser));
        for fixture in checked {
            assert_eq!(
                std::fs::read_to_string(&fixture).unwrap_or_default(),
                ErrorKind::table_json(),
                "{} is stale: run `cargo test -p featherweight-runtime --lib -- \
                 --ignored regenerate_error_kinds`, then update the browser host",
                fixture.display()
            );
        }
    }

    #[test]
    #[ignore = "regenerates committed fixtures; run by hand"]
    fn regenerate_error_kinds() {
        for fixture in error_kind_fixtures() {
            std::fs::write(fixture, ErrorKind::table_json()).unwrap();
        }
    }

    #[test]
    fn error_responses_decode_structurally() {
        assert!(matches!(
            decode_read_response(error_to_response(&Error::cancelled("stop"))),
            Err(Error::Cancelled { .. })
        ));
        assert!(matches!(
            decode_read_response(error_to_response(&Error::resource_limit("large"))),
            Err(Error::ResourceLimit { .. })
        ));
        let response = error_to_response(&Error::permission_denied("no capability"));
        let err = decode_write_response(response).unwrap_err();
        assert!(matches!(err, Error::PermissionDenied { .. }));

        let response = error_to_response(&Error::overloaded("busy"));
        let err = decode_read_response(response).unwrap_err();
        assert!(matches!(err, Error::Overloaded { .. }));
    }

    #[test]
    fn implementation_details_do_not_leak_through_unavailable() {
        // The abstraction rule: a crashed block shows as "unavailable",
        // never as its internal error text.
        let response = error_to_response(&Error::overloaded("cache block crashed horribly"));
        match response {
            Value::Map(map) => match map.get("error") {
                Some(Value::Map(error)) => {
                    assert_eq!(error.get("type"), Some(&Value::from("unavailable")));
                    let message = match error.get("message") {
                        Some(Value::String(m)) => m.clone(),
                        _ => panic!(),
                    };
                    assert!(!message.contains("crashed"));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }
}
