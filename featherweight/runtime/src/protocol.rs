//! Server-protocol envelopes (`isotope/spec/07-server-protocol.md`).
//!
//! Requests flow runtime -> block as `{op, path, data, respond_to}`;
//! responses flow block -> runtime as `{result, value?, path?, error?}`.
//! This module is the single place both shapes are encoded and decoded —
//! blocks use it to serve, the runtime uses it to call.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, Value};

/// A request as decoded by a serving block.
#[derive(Debug, Clone, PartialEq)]
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
/// (`isotope/spec/09-posix-closure.md`).
#[derive(Debug, Clone, PartialEq)]
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

/// Build a successful read response: `{result: "ok", value}`.
pub fn ok_value(value: Value) -> Value {
    let mut map = BTreeMap::new();
    map.insert("result".to_string(), Value::from("ok"));
    map.insert("value".to_string(), value);
    Value::Map(map)
}

/// Build a successful write response: `{result: "ok", path}`.
pub fn ok_path(path: &Path) -> Value {
    let mut map = BTreeMap::new();
    map.insert("result".to_string(), Value::from("ok"));
    map.insert("path".to_string(), Value::String(path.to_string()));
    Value::Map(map)
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
/// able to tell what implementation is behind the path.
pub fn error_to_response(error: &Error) -> Value {
    match error {
        Error::NoRoute { .. } | Error::NotFound { .. } => {
            err_response("not_found", &error.to_string(), false)
        }
        Error::Path(_) => err_response("invalid_path", &error.to_string(), false),
        Error::PermissionDenied { .. } => err_response("forbidden", &error.to_string(), false),
        Error::Overloaded { .. } | Error::Cancelled { .. } => {
            err_response("unavailable", "store temporarily unavailable", true)
        }
        Error::DeadlineExceeded { .. } => err_response("timeout", &error.to_string(), true),
        Error::Conflict { .. } => err_response("conflict", &error.to_string(), false),
        _ => err_response("store_error", &error.to_string(), false),
    }
}

fn decode_error(map: &BTreeMap<String, Value>) -> Error {
    let (error_type, message) = match map.get("error") {
        Some(Value::Map(error)) => (
            match error.get("type") {
                Some(Value::String(t)) => t.as_str(),
                _ => "store_error",
            },
            match error.get("message") {
                Some(Value::String(m)) => m.clone(),
                _ => "unknown error".to_string(),
            },
        ),
        _ => ("store_error", "malformed error response".to_string()),
    };
    match error_type {
        "not_found" => Error::store("server_protocol", "call", format!("not found: {}", message)),
        "invalid_path" => Error::store("server_protocol", "call", message),
        "forbidden" | "not_readable" | "not_writable" => Error::permission_denied(message),
        "unavailable" => Error::overloaded(message),
        "timeout" => Error::deadline_exceeded(message),
        "conflict" => Error::conflict(message),
        _ => Error::store("server_protocol", "call", message),
    }
}

/// Decode a response to a read: `Ok(None)` for a Null value.
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
        Some(Value::String(result)) if result == "ok" => {
            match map.get("value").cloned().unwrap_or(Value::Null) {
                Value::Null => Ok(None),
                value => Ok(Some(value)),
            }
        }
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
mod tests {
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
        assert_eq!(decode_read_response(ok_value(Value::Null)).unwrap(), None);
    }

    #[test]
    fn write_response_round_trip() {
        assert_eq!(
            decode_write_response(ok_path(&path!("outstanding/1"))).unwrap(),
            path!("outstanding/1")
        );
    }

    #[test]
    fn error_responses_decode_structurally() {
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
        let response = error_to_response(&Error::cancelled("cache block crashed horribly"));
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
