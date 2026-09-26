//! Pieces shared by the two HTTP brokers.
//!
//! [`crate::HttpBrokerStore`] and [`crate::BackgroundHttpBrokerStore`] serve
//! the same surface — a root of references, a `docs` lens, a `meta` lens and
//! an `outstanding/{id}` collection — over different execution models. The
//! surface lives here once; each broker supplies only its handle ids and a
//! per-handle [`HandleMeta`] snapshot.

use collection_literals::btree;

use structfs_core_store::{Error, Path, Record, Reference, Value};

pub(crate) const OUTSTANDING_PREFIX: &str = "outstanding";
pub(crate) const DOCS_PATH: &str = "docs";
pub(crate) const META_PATH: &str = "meta";

pub(crate) type RequestId = u64;

/// Handle path for an outstanding request: `outstanding/{id}`.
pub(crate) fn outstanding_path(request_id: RequestId) -> Path {
    Path::from_components(vec![OUTSTANDING_PREFIX.to_string(), request_id.to_string()])
}

/// Which broker a shared builder is serving.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerKind {
    /// Executes on read, blocking the caller.
    Blocking,
    /// Executes on a background thread; adds the `response/wait` lens.
    Background,
}

impl BrokerKind {
    fn is_background(self) -> bool {
        self == BrokerKind::Background
    }
}

/// A failure remembered on a handle so repeated reads stay idempotent.
///
/// [`structfs_core_store::Error`] is not `Clone` (it can wrap an
/// `io::Error`), so a broker cannot simply keep the error it got. It keeps
/// the *typed shape* plus the message instead and rebuilds an equivalent
/// error on every read — a timeout stays `DeadlineExceeded` on the second
/// read as much as the first.
#[derive(Clone)]
pub(crate) struct CachedFailure {
    kind: FailureKind,
    message: String,
}

#[derive(Clone, Copy)]
enum FailureKind {
    Store,
    DeadlineExceeded,
    Overloaded,
    PermissionDenied,
    InvalidArgument,
    Conflict,
    ResourceLimit,
    Cancelled,
    Decode,
}

impl CachedFailure {
    /// Convert an executor failure once, remembering its typed shape.
    pub(crate) fn new(error: crate::Error) -> Self {
        let core = Error::from(error);
        let message = core.to_string();
        let kind = match core {
            Error::DeadlineExceeded { .. } => FailureKind::DeadlineExceeded,
            Error::Overloaded { .. } => FailureKind::Overloaded,
            Error::PermissionDenied { .. } => FailureKind::PermissionDenied,
            Error::InvalidArgument { .. } => FailureKind::InvalidArgument,
            Error::Conflict { .. } => FailureKind::Conflict,
            Error::ResourceLimit { .. } => FailureKind::ResourceLimit,
            Error::Cancelled { .. } => FailureKind::Cancelled,
            Error::Codec {
                operation: structfs_core_store::CodecOperation::Decode,
                ..
            } => FailureKind::Decode,
            // `NotFound` carries a store path the HTTP layer does not have,
            // and every remaining variant has no closer typed equivalent.
            _ => FailureKind::Store,
        };
        Self { kind, message }
    }

    /// The failure message, for `RequestStatus::failed`.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// Rebuild an equivalent typed error.
    pub(crate) fn to_error(&self) -> Error {
        let message = format!("HTTP request failed: {}", self.message);
        match self.kind {
            FailureKind::DeadlineExceeded => Error::deadline_exceeded(message),
            FailureKind::Overloaded => Error::overloaded(message),
            FailureKind::PermissionDenied => Error::permission_denied(message),
            FailureKind::InvalidArgument => Error::invalid_argument(message),
            FailureKind::Conflict => Error::conflict(message),
            FailureKind::ResourceLimit => Error::resource_limit(message),
            FailureKind::Cancelled => Error::Cancelled { message },
            FailureKind::Decode => Error::decode(structfs_core_store::Format::JSON, message),
            FailureKind::Store => Error::store("http_broker", "read", message),
        }
    }
}

/// Navigate into a value with the remaining path components.
///
/// One implementation for every lens in the crate, over
/// [`Value::get`]. A component that does not resolve reads as absent, per
/// the [`structfs_core_store::Reader::read`] contract.
pub(crate) fn navigate(value: Value, sub: &Path) -> Option<Value> {
    if sub.is_empty() {
        return Some(value);
    }
    value.get(sub).cloned()
}

/// Documentation for a broker store.
pub(crate) fn broker_docs(kind: BrokerKind) -> Value {
    let mut paths = btree! {
        "write /".into() => Value::String("Queue request, returns outstanding/{id}".into()),
        "read /outstanding".into() => Value::String("List queued requests as {items: [references]}".into()),
        "read /outstanding/{id}/request".into() => Value::String("View the queued request".into()),
        "write /outstanding/{id} null".into() => Value::String("Delete the handle".into()),
    };

    let (title, description, example): (&str, &str, Vec<&str>) = if kind.is_background() {
        paths.insert(
            "read /outstanding/{id}".into(),
            Value::String("Get request status (pending/complete/failed)".into()),
        );
        paths.insert(
            "read /outstanding/{id}/response".into(),
            Value::String("Get response (absent if still pending)".into()),
        );
        paths.insert(
            "read /outstanding/{id}/response/wait".into(),
            Value::String("Block until response ready".into()),
        );
        (
            "Background HTTP Broker",
            "Queue HTTP requests by writing; requests execute on background threads.",
            vec![
                "write / {\"method\": \"GET\", \"path\": \"https://httpbin.org/json\"}",
                "# Returns: outstanding/0 (request starts executing)",
                "read /outstanding/0",
                "# Returns status: {\"state\": \"pending\"} or {\"state\": \"complete\"}",
                "read /outstanding/0/response/wait",
                "# Blocks until complete, returns response",
            ],
        )
    } else {
        paths.insert(
            "read /outstanding/{id}".into(),
            Value::String("Execute request (blocks) and return response".into()),
        );
        paths.insert(
            "read /outstanding/{id}/response/body".into(),
            Value::String("Navigate into response fields".into()),
        );
        (
            "Sync HTTP Broker",
            "Queue HTTP requests by writing, execute on read (blocks until complete).",
            vec![
                "write / {\"method\": \"GET\", \"path\": \"https://httpbin.org/json\"}",
                "# Returns: outstanding/0",
                "read /outstanding/0",
                "# Blocks until complete, returns response",
            ],
        )
    };

    Value::Map(btree! {
        "title".into() => Value::String(title.into()),
        "description".into() => Value::String(description.into()),
        "paths".into() => Value::Map(paths),
        "example".into() => Value::Array(
            example.into_iter().map(|line| Value::String(line.into())).collect(),
        ),
    })
}

/// The reference map served at a broker's root.
pub(crate) fn root_references() -> Value {
    Value::Map(btree! {
        "outstanding".into() => Reference::with_type(OUTSTANDING_PREFIX, "collection").to_value(),
        "queue".into() => Reference::with_type("meta/queue", "action").to_value(),
        "meta".into() => Reference::with_type(META_PATH, "meta").to_value(),
        "docs".into() => Reference::with_type(DOCS_PATH, "docs").to_value(),
    })
}

/// `{items: [references]}` for a collection of handle ids.
fn listing(ids: &[RequestId], path: impl Fn(RequestId) -> String, type_name: &str) -> Value {
    let items: Vec<Value> = ids
        .iter()
        .map(|id| Reference::with_type(path(*id), type_name).to_value())
        .collect();
    Value::Map(btree! { "items".into() => Value::Array(items) })
}

/// The listing served at `outstanding`.
pub(crate) fn outstanding_listing(ids: &[RequestId]) -> Value {
    listing(
        ids,
        |id| format!("{}/{}", OUTSTANDING_PREFIX, id),
        "request-handle",
    )
}

/// Action descriptor for queuing a request.
fn queue_action_descriptor() -> Value {
    Value::Map(btree! {
        "type".into() => Value::Map(btree! { "name".into() => Value::String("action".into()) }),
        "method".into() => Value::String("write".into()),
        "target".into() => Reference::new("").to_value(),
        "accepts".into() => Value::Map(btree! {
            "method".into() => Value::Map(btree! {
                "type".into() => Value::Map(btree! { "name".into() => Value::String("string".into()) }),
                "required".into() => Value::Bool(true),
                "values".into() => Value::Array(vec![
                    Value::String("GET".into()),
                    Value::String("POST".into()),
                    Value::String("PUT".into()),
                    Value::String("PATCH".into()),
                    Value::String("DELETE".into()),
                    Value::String("HEAD".into()),
                    Value::String("OPTIONS".into()),
                    Value::String("CONNECT".into()),
                    Value::String("TRACE".into()),
                ]),
            }),
            "path".into() => Value::Map(btree! {
                "type".into() => Value::Map(btree! { "name".into() => Value::String("string".into()) }),
                "required".into() => Value::Bool(true),
            }),
            "headers".into() => Value::Map(btree! {
                "type".into() => Value::Map(btree! { "name".into() => Value::String("map".into()) }),
                "required".into() => Value::Bool(false),
            }),
            "body".into() => Value::Map(btree! {
                "type".into() => Value::Map(btree! { "name".into() => Value::String("string".into()) }),
                "required".into() => Value::Bool(false),
            }),
        }),
        "returns".into() => Value::Map(btree! {
            "type".into() => Value::Map(btree! { "name".into() => Value::String("request-handle".into()) }),
            "collection".into() => Reference::new(OUTSTANDING_PREFIX).to_value(),
        }),
    })
}

/// Action descriptor for deleting a handle.
fn delete_action_descriptor(id: RequestId) -> Value {
    Value::Map(btree! {
        "type".into() => Value::Map(btree! { "name".into() => Value::String("action".into()) }),
        "method".into() => Value::String("write".into()),
        "target".into() => Reference::new(format!("outstanding/{}", id)).to_value(),
        "accepts".into() => Value::String("null".into()),
        "returns".into() => Value::String("void".into()),
    })
}

/// A handle's state as the `meta` lens reports it.
pub(crate) struct HandleMeta {
    /// `"pending"`, `"complete"` or `"failed"`.
    pub status: &'static str,
    /// The request method, rendered for display.
    pub method: String,
    /// The request URL.
    pub url: String,
}

/// Serve the `meta/...` lens for either broker.
///
/// `ids` are the live handle ids; `meta_of` answers for one of them. An
/// unknown id, and any other well-formed path that names nothing
/// (`meta/unknown`, `meta/queue/x`, `meta/outstanding/0/bogus`), reads as
/// absent. Only a non-numeric handle id is `InvalidArgument`.
pub(crate) fn read_meta(
    kind: BrokerKind,
    path: &Path,
    ids: &[RequestId],
    meta_of: impl Fn(RequestId) -> Option<HandleMeta>,
) -> Result<Option<Record>, Error> {
    // meta — the available meta operations.
    if path.len() == 1 {
        return Ok(Some(Record::parsed(Value::Map(btree! {
            "queue".into() => Reference::with_type("meta/queue", "action").to_value(),
            "outstanding".into() => Reference::with_type("meta/outstanding", "collection").to_value(),
        }))));
    }

    if path.len() == 2 && &path[1] == "queue" {
        return Ok(Some(Record::parsed(queue_action_descriptor())));
    }

    if &path[1] == OUTSTANDING_PREFIX {
        // meta/outstanding — handles with meta references.
        if path.len() == 2 {
            return Ok(Some(Record::parsed(listing(
                ids,
                |id| format!("meta/outstanding/{}", id),
                "request-handle-meta",
            ))));
        }

        let id: RequestId = path[2]
            .parse()
            .map_err(|_| Error::invalid_argument(format!("Invalid handle ID: {}", &path[2])))?;

        let Some(meta) = meta_of(id) else {
            return Ok(None);
        };

        // meta/outstanding/{id}/delete — the delete action descriptor.
        if path.len() == 4 && &path[3] == "delete" {
            return Ok(Some(Record::parsed(delete_action_descriptor(id))));
        }

        // meta/outstanding/{id} — handle state plus navigation.
        if path.len() == 3 {
            let mut map = btree! {
                "state".into() => Value::Map(btree! {
                    "status".into() => Value::String(meta.status.to_string()),
                    "method".into() => Value::String(meta.method),
                    "url".into() => Value::String(meta.url),
                }),
                "request".into() => Reference::with_type(format!("outstanding/{}/request", id), "http-request").to_value(),
                "response".into() => Reference::with_type(format!("outstanding/{}/response", id), "http-response").to_value(),
                "delete".into() => Reference::with_type(format!("meta/outstanding/{}/delete", id), "action").to_value(),
            };
            if kind.is_background() {
                map.insert(
                    "wait".into(),
                    Reference::with_type(format!("outstanding/{}/response/wait", id), "accessor")
                        .to_value(),
                );
            }
            return Ok(Some(Record::parsed(Value::Map(map))));
        }
    }

    // Any other well-formed path under `meta` simply names nothing: absent,
    // per the `Reader::read` contract. Only a non-numeric handle id (above)
    // is a malformed request.
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    fn meta(status: &'static str) -> HandleMeta {
        HandleMeta {
            status,
            method: "GET".into(),
            url: "https://example.com".into(),
        }
    }

    #[test]
    fn navigate_uses_value_get_semantics() {
        let value = Value::Map(btree! {
            "a".into() => Value::Array(vec![Value::Integer(1), Value::Integer(2)]),
        });
        assert_eq!(
            navigate(value.clone(), &path!("a/1")),
            Some(Value::Integer(2))
        );
        assert_eq!(navigate(value.clone(), &path!("")), Some(value.clone()));
        assert_eq!(navigate(value.clone(), &path!("a/9")), None);
        assert_eq!(navigate(value, &path!("missing")), None);
    }

    #[test]
    fn unknown_handle_reads_as_absent_in_the_meta_lens() {
        let result = read_meta(
            BrokerKind::Blocking,
            &path!("meta/outstanding/9"),
            &[],
            |_| None,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn invalid_handle_id_is_an_argument_error() {
        let error = read_meta(
            BrokerKind::Blocking,
            &path!("meta/outstanding/abc"),
            &[],
            |_| None,
        )
        .unwrap_err();
        assert!(matches!(error, Error::InvalidArgument { .. }));
        assert!(error.to_string().contains("Invalid handle ID"));
    }

    #[test]
    fn only_the_background_broker_advertises_wait() {
        for (kind, expected) in [
            (BrokerKind::Blocking, false),
            (BrokerKind::Background, true),
        ] {
            let record = read_meta(kind, &path!("meta/outstanding/0"), &[0], |_| {
                Some(meta("pending"))
            })
            .unwrap()
            .unwrap();
            let Value::Map(map) = record.as_value().unwrap() else {
                panic!("expected map");
            };
            assert_eq!(map.contains_key("wait"), expected);
        }
    }

    #[test]
    fn unknown_well_formed_meta_paths_read_as_absent() {
        for missing in ["meta/unknown", "meta/queue/x", "meta/outstanding/0/bogus"] {
            let result = read_meta(
                BrokerKind::Blocking,
                &Path::parse(missing).unwrap(),
                &[0],
                |_| Some(meta("pending")),
            )
            .unwrap();
            assert!(result.is_none(), "{missing}");
        }
    }

    #[test]
    fn cached_failures_keep_their_typed_shape() {
        let timeout = CachedFailure::new(crate::Error::Status {
            status: 504,
            status_text: "Gateway Timeout".into(),
            message: "slow".into(),
            path: None,
        });
        assert!(matches!(timeout.to_error(), Error::DeadlineExceeded { .. }));
        // Rebuilding is repeatable: the second read sees the same shape.
        assert!(matches!(timeout.to_error(), Error::DeadlineExceeded { .. }));
        assert!(timeout.message().contains("slow"));

        let denied = CachedFailure::new(crate::Error::status(403, "Forbidden", "nope"));
        assert!(matches!(denied.to_error(), Error::PermissionDenied { .. }));

        let other = CachedFailure::new(crate::Error::Other {
            message: "Connection refused".into(),
        });
        assert!(matches!(other.to_error(), Error::Store { .. }));
        assert!(other.to_error().to_string().contains("Connection refused"));
    }

    #[test]
    fn docs_describe_the_listing_envelope() {
        for kind in [BrokerKind::Blocking, BrokerKind::Background] {
            let Value::Map(docs) = broker_docs(kind) else {
                panic!("expected map");
            };
            let Some(Value::Map(paths)) = docs.get("paths") else {
                panic!("expected paths");
            };
            let listing = paths.get("read /outstanding").unwrap();
            assert_eq!(
                listing,
                &Value::String("List queued requests as {items: [references]}".into())
            );
        }
    }
}
