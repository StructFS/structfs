//! Tests for the blocking HTTP broker. Every request goes through
//! `MockExecutor`; nothing here touches the network.

use super::*;
use crate::executor::mock::MockExecutor;
use crate::types::Method;
use structfs_core_store::{path, Reference};

fn broker(executor: MockExecutor) -> HttpBrokerStore<MockExecutor> {
    HttpBrokerStore::with_executor(executor)
}

fn queue(store: &mut HttpBrokerStore<MockExecutor>, request: HttpRequest) -> Path {
    store
        .write(&path!(""), Record::parsed(to_value(&request).unwrap()))
        .unwrap()
}

fn value_of(record: Record) -> Value {
    record.into_value(&NoCodec).unwrap()
}

fn items(store: &mut HttpBrokerStore<MockExecutor>, at: &Path) -> Vec<Value> {
    let Value::Map(map) = value_of(store.read(at).unwrap().unwrap()) else {
        panic!("expected a map");
    };
    let Some(Value::Array(items)) = map.get("items") else {
        panic!("expected an items array");
    };
    items.clone()
}

#[test]
fn parse_handle_path_splits_id_from_sub_path() {
    let parse = HttpBrokerStore::<MockExecutor>::parse_handle_path;
    assert!(parse(&path!("outstanding")).is_none());
    assert!(parse(&path!("other/123")).is_none());
    assert!(parse(&path!("outstanding/abc")).is_none());
    assert!(parse(&path!("")).is_none());

    assert_eq!(parse(&path!("outstanding/0")), Some((0, path!(""))));
    assert_eq!(parse(&path!("outstanding/123")), Some((123, path!(""))));
    assert_eq!(
        parse(&path!("outstanding/0/request")),
        Some((0, path!("request")))
    );
    assert_eq!(
        parse(&path!("outstanding/0/response/status")),
        Some((0, path!("response/status")))
    );
}

#[test]
fn queueing_returns_a_handle_path() {
    let mut store = broker(MockExecutor::new());
    let handle = queue(&mut store, HttpRequest::get("https://api.test/thing"));
    assert_eq!(handle.to_string(), "outstanding/0");
    assert_eq!(store.handle_count(), 1);
}

#[test]
fn reading_a_handle_executes_through_the_executor() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/users",
        MockExecutor::success_response(serde_json::json!({"users": ["alice", "bob"]})),
    );
    let mut store = broker(mock);

    let handle = queue(&mut store, HttpRequest::get("https://api.test/users"));
    let response: HttpResponse =
        from_value(value_of(store.read(&handle).unwrap().unwrap())).unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body,
        serde_json::json!({"users": ["alice", "bob"]})
    );
}

#[test]
fn handles_are_independent_and_execute_in_read_order() {
    let mock = MockExecutor::new()
        .with_response(
            "/a",
            MockExecutor::success_response(serde_json::json!({"id": "a"})),
        )
        .with_response(
            "/b",
            MockExecutor::success_response(serde_json::json!({"id": "b"})),
        );
    let mut store = broker(mock);

    let h1 = queue(&mut store, HttpRequest::get("/a"));
    let h2 = queue(&mut store, HttpRequest::get("/b"));
    assert_eq!(h1.to_string(), "outstanding/0");
    assert_eq!(h2.to_string(), "outstanding/1");
    assert_eq!(store.handle_count(), 2);

    let r2: HttpResponse = from_value(value_of(store.read(&h2).unwrap().unwrap())).unwrap();
    assert_eq!(r2.body, serde_json::json!({"id": "b"}));
    let r1: HttpResponse = from_value(value_of(store.read(&h1).unwrap().unwrap())).unwrap();
    assert_eq!(r1.body, serde_json::json!({"id": "a"}));
}

#[test]
fn unknown_handles_read_as_absent() {
    // Matches the background broker and the `Reader::read` contract: a
    // missing path is `Ok(None)`, not an error.
    let mut store = broker(MockExecutor::new());
    assert!(store.read(&path!("outstanding/999")).unwrap().is_none());
}

#[test]
fn paths_outside_the_store_surface_are_argument_errors() {
    let mut store = broker(MockExecutor::new());
    let error = store.read(&path!("invalid")).unwrap_err();
    assert!(matches!(error, Error::InvalidArgument { .. }));
    assert!(error.to_string().contains("Invalid path"));
}

#[test]
fn executor_failures_are_reported_and_cached() {
    let mut store = broker(MockExecutor::new().fail_with("Connection refused"));
    let handle = queue(&mut store, HttpRequest::get("https://api.test/thing"));

    for _ in 0..2 {
        let error = store.read(&handle).unwrap_err();
        assert!(error.to_string().contains("Connection refused"), "{error}");
    }
}

#[test]
fn a_root_write_that_is_not_a_request_is_rejected() {
    let mut store = broker(MockExecutor::new());
    let result = store.write(
        &path!(""),
        Record::parsed(to_value(&"not a request").unwrap()),
    );
    // A parsed value of the wrong shape is a caller argument error, not a
    // codec failure.
    assert!(matches!(result, Err(Error::InvalidArgument { .. })));

    // `deny_unknown_fields` means an arbitrary map is no longer silently
    // accepted as `GET ""`.
    let result = store.write(
        &path!(""),
        Record::parsed(to_value(&serde_json::json!({"name": "Bob"})).unwrap()),
    );
    assert!(matches!(result, Err(Error::InvalidArgument { .. })));
}

#[test]
fn reads_are_idempotent() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/data",
        MockExecutor::success_response(serde_json::json!({"value": 42})),
    );
    let mut store = broker(mock);
    let handle = queue(&mut store, HttpRequest::get("https://api.test/data"));

    let first = value_of(store.read(&handle).unwrap().unwrap());
    let second = value_of(store.read(&handle).unwrap().unwrap());
    assert_eq!(first, second);
    assert!(store.has_handle(0));
}

#[test]
fn outstanding_lists_references() {
    let mut store = broker(MockExecutor::new());
    for path in ["/a", "/b", "/c"] {
        queue(&mut store, HttpRequest::get(path));
    }

    let listed = items(&mut store, &path!("outstanding"));
    assert_eq!(listed.len(), 3);
    for item in &listed {
        assert!(Reference::from_value(item).is_some());
    }
}

#[test]
fn the_queued_request_is_readable() {
    let mut store = broker(MockExecutor::new());
    let handle = queue(
        &mut store,
        HttpRequest::get("https://api.test/users").with_header("Authorization", "Bearer token123"),
    );

    let record = store
        .read(&handle.join(&path!("request")))
        .unwrap()
        .unwrap();
    let retrieved: HttpRequest = from_value(value_of(record)).unwrap();

    assert_eq!(retrieved.method, Method::GET);
    assert_eq!(retrieved.path, "https://api.test/users");
    assert_eq!(
        retrieved.headers.get("Authorization"),
        Some(&"Bearer token123".to_string())
    );
}

#[test]
fn unknown_sub_paths_are_argument_errors() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/test"));

    let error = store.read(&path!("outstanding/0/unknown")).unwrap_err();
    assert!(matches!(error, Error::InvalidArgument { .. }));
    assert!(error.to_string().contains("Unknown sub-path"));
}

#[test]
fn writing_null_deletes_a_handle() {
    let mock = MockExecutor::new().with_response(
        "/test",
        MockExecutor::success_response(serde_json::json!({"ok": true})),
    );
    let mut store = broker(mock);
    let handle = queue(&mut store, HttpRequest::get("/test"));
    store.read(&handle).unwrap();
    assert!(store.has_handle(0));

    store.write(&handle, Record::parsed(Value::Null)).unwrap();

    assert!(!store.has_handle(0));
    assert!(store.read(&handle).unwrap().is_none());
}

#[test]
fn deleting_updates_the_listing() {
    let mut store = broker(MockExecutor::new());
    let h1 = queue(&mut store, HttpRequest::get("/a"));
    queue(&mut store, HttpRequest::get("/b"));

    store.write(&h1, Record::parsed(Value::Null)).unwrap();

    let listed = items(&mut store, &path!("outstanding"));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        Reference::from_value(&listed[0]).unwrap().path,
        "outstanding/1"
    );
}

#[test]
fn a_queued_request_cannot_be_overwritten() {
    let mut store = broker(MockExecutor::new());
    let handle = queue(&mut store, HttpRequest::get("/original"));

    let error = store
        .write(
            &handle,
            Record::parsed(to_value(&HttpRequest::get("/replacement")).unwrap()),
        )
        .unwrap_err();
    // Same structural answer as the background broker's handle store.
    assert!(matches!(error, Error::Conflict { .. }));
    assert!(error.to_string().contains("Cannot overwrite"));
}

#[test]
fn writes_outside_the_store_surface_are_argument_errors() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/test"));
    for bad in ["something/else", "outstanding/0/request"] {
        let error = store
            .write(
                &Path::parse(bad).unwrap(),
                Record::parsed(to_value(&HttpRequest::get("/test")).unwrap()),
            )
            .unwrap_err();
        assert!(error.to_string().contains("Invalid write path"), "{bad}");
    }
}

#[test]
fn responses_and_requests_can_be_navigated() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/data",
        MockExecutor::success_response(serde_json::json!({"nested": {"value": 42}})),
    );
    let mut store = broker(mock);
    let handle = queue(&mut store, HttpRequest::get("https://api.test/data"));

    let status = value_of(
        store
            .read(&handle.join(&path!("response/status")))
            .unwrap()
            .unwrap(),
    );
    assert_eq!(status, Value::Integer(200));

    let nested = value_of(
        store
            .read(&handle.join(&path!("response/body/nested/value")))
            .unwrap()
            .unwrap(),
    );
    assert_eq!(nested, Value::Integer(42));

    let method = value_of(
        store
            .read(&handle.join(&path!("request/method")))
            .unwrap()
            .unwrap(),
    );
    assert_eq!(method, Value::String("GET".to_string()));
}

#[test]
fn navigating_to_a_missing_field_reads_as_absent() {
    let mock = MockExecutor::new().with_response(
        "/test",
        MockExecutor::success_response(serde_json::json!({"a": 1})),
    );
    let mut store = broker(mock);
    let handle = queue(&mut store, HttpRequest::get("/test"));

    assert!(store
        .read(&handle.join(&path!("response/body/nonexistent/path")))
        .unwrap()
        .is_none());
    assert!(store
        .read(&handle.join(&path!("request/nonexistent")))
        .unwrap()
        .is_none());
}

// ==================== docs and meta ====================

#[test]
fn docs_describe_the_store() {
    let mut store = broker(MockExecutor::new());
    let Value::Map(docs) = value_of(store.read(&path!("docs")).unwrap().unwrap()) else {
        panic!("expected a map");
    };
    for key in ["title", "description", "paths", "example"] {
        assert!(docs.contains_key(key), "missing {key}");
    }
}

#[test]
fn the_root_is_a_map_of_references() {
    let mut store = broker(MockExecutor::new());
    let Value::Map(root) = value_of(store.read(&path!("")).unwrap().unwrap()) else {
        panic!("expected a map");
    };
    for key in ["outstanding", "queue", "meta", "docs"] {
        assert!(
            Reference::from_value(root.get(key).unwrap()).is_some(),
            "{key}"
        );
    }
}

#[test]
fn the_meta_root_lists_meta_operations() {
    let mut store = broker(MockExecutor::new());
    let Value::Map(meta) = value_of(store.read(&path!("meta")).unwrap().unwrap()) else {
        panic!("expected a map");
    };
    assert!(Reference::from_value(meta.get("queue").unwrap()).is_some());
    assert!(Reference::from_value(meta.get("outstanding").unwrap()).is_some());
}

#[test]
fn the_queue_action_descriptor_is_served() {
    let mut store = broker(MockExecutor::new());
    let Value::Map(action) = value_of(store.read(&path!("meta/queue")).unwrap().unwrap()) else {
        panic!("expected a map");
    };
    let Some(Value::Map(type_info)) = action.get("type") else {
        panic!("expected type info");
    };
    assert_eq!(
        type_info.get("name"),
        Some(&Value::String("action".to_string()))
    );
    assert_eq!(
        action.get("method"),
        Some(&Value::String("write".to_string()))
    );
    assert!(Reference::from_value(action.get("target").unwrap()).is_some());
    let Some(Value::Map(accepts)) = action.get("accepts") else {
        panic!("expected accepts map");
    };
    assert!(accepts.contains_key("method"));
    assert!(accepts.contains_key("path"));
}

#[test]
fn the_meta_listing_references_handle_meta() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/test"));

    let listed = items(&mut store, &path!("meta/outstanding"));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        Reference::from_value(&listed[0]).unwrap().path,
        "meta/outstanding/0"
    );
}

#[test]
fn handle_meta_reports_state_and_navigation() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("https://api.test/thing"));

    let Value::Map(meta) = value_of(store.read(&path!("meta/outstanding/0")).unwrap().unwrap())
    else {
        panic!("expected a map");
    };
    let Some(Value::Map(state)) = meta.get("state") else {
        panic!("expected a state map");
    };
    assert_eq!(
        state.get("status"),
        Some(&Value::String("pending".to_string()))
    );
    for key in ["request", "response", "delete"] {
        assert!(
            Reference::from_value(meta.get(key).unwrap()).is_some(),
            "{key}"
        );
    }
    // The blocking broker has no parked wait to advertise.
    assert!(!meta.contains_key("wait"));
}

#[test]
fn the_delete_action_descriptor_targets_the_handle() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/test"));

    let Value::Map(action) = value_of(
        store
            .read(&path!("meta/outstanding/0/delete"))
            .unwrap()
            .unwrap(),
    ) else {
        panic!("expected a map");
    };
    let target = Reference::from_value(action.get("target").unwrap()).unwrap();
    assert_eq!(target.path, "outstanding/0");
    assert_eq!(
        action.get("accepts"),
        Some(&Value::String("null".to_string()))
    );
}

#[test]
fn meta_rejects_bad_ids_and_reads_unknown_paths_as_absent() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/test"));

    for missing in ["meta/unknown", "meta/queue/x", "meta/outstanding/0/bogus"] {
        assert!(
            store
                .read(&Path::parse(missing).unwrap())
                .unwrap()
                .is_none(),
            "{missing}"
        );
    }

    let bad_id = store.read(&path!("meta/outstanding/abc")).unwrap_err();
    assert!(bad_id.to_string().contains("Invalid handle ID"));

    // An id that is simply not there reads as absent.
    assert!(store
        .read(&path!("meta/outstanding/999"))
        .unwrap()
        .is_none());
}

#[test]
fn writes_at_or_below_an_unknown_handle_are_not_found() {
    let mut store = broker(MockExecutor::new());
    let request = Record::parsed(to_value(&HttpRequest::get("/x")).unwrap());
    for target in ["outstanding/99", "outstanding/99/request"] {
        let error = store
            .write(&Path::parse(target).unwrap(), request.clone())
            .unwrap_err();
        assert!(
            matches!(error, Error::NotFound { .. }),
            "{target}: {error:?}"
        );
    }
    // Releasing an unknown handle stays an idempotent no-op.
    store
        .write(&path!("outstanding/99"), Record::parsed(Value::Null))
        .unwrap();

    // Below a *live* handle there is nothing writable.
    queue(&mut store, HttpRequest::get("/x"));
    let error = store
        .write(&path!("outstanding/0/request"), request)
        .unwrap_err();
    assert!(matches!(error, Error::InvalidArgument { .. }), "{error:?}");
}

/// Malformed, relative and non-http(s) URLs are rejected by the real
/// executor *before* any network I/O, as `InvalidArgument`.
#[test]
fn malformed_request_urls_are_invalid_arguments() {
    let mut store = HttpBrokerStore::with_default_timeout().unwrap();
    for bad in ["not a url", "/x", "ftp://api.test/file"] {
        let handle = store
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get(bad)).unwrap()),
            )
            .unwrap();
        let error = store.read(&handle).unwrap_err();
        assert!(
            matches!(error, Error::InvalidArgument { .. }),
            "{bad}: {error:?}"
        );
        // Cached: the second read reports the same typed error.
        assert!(matches!(
            store.read(&handle).unwrap_err(),
            Error::InvalidArgument { .. }
        ));
    }
}

#[test]
fn constructing_with_the_default_executor_works() {
    assert!(HttpBrokerStore::with_default_timeout().is_ok());
    assert!(HttpBrokerStore::new(Duration::from_secs(5)).is_ok());
}
