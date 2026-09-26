//! Tests for the background HTTP broker. Every request goes through
//! `MockExecutor`; nothing here touches the network.

use super::*;
use crate::executor::mock::MockExecutor;
use crate::types::{HttpRequest, HttpResponse, Method};
use structfs_core_store::{path, NoCodec, Reference, Value};
use structfs_serde_store::{from_value, to_value};

fn broker(executor: MockExecutor) -> BackgroundHttpBrokerStore<MockExecutor> {
    BackgroundHttpBrokerStore::with_executor(executor, Duration::from_secs(5)).unwrap()
}

fn queue(store: &mut BackgroundHttpBrokerStore<MockExecutor>, request: HttpRequest) -> Path {
    store
        .write(&path!(""), Record::parsed(to_value(&request).unwrap()))
        .unwrap()
}

fn value_of(record: Record) -> Value {
    record.into_value(&NoCodec).unwrap()
}

fn items(store: &mut BackgroundHttpBrokerStore<MockExecutor>, at: &Path) -> Vec<Value> {
    let Value::Map(map) = value_of(store.read(at).unwrap().unwrap()) else {
        panic!("expected a map");
    };
    let Some(Value::Array(items)) = map.get("items") else {
        panic!("expected an items array");
    };
    items.clone()
}

#[test]
fn paths_outside_the_store_surface_are_rejected() {
    let mut store = broker(MockExecutor::new());
    for bad in ["other/123", "outstanding/abc", "invalid"] {
        let error = store.read(&Path::parse(bad).unwrap()).unwrap_err();
        assert!(matches!(error, Error::InvalidArgument { .. }), "{bad}");
        assert!(error.to_string().contains("Invalid path"), "{bad}");
    }
}

#[test]
fn queueing_starts_the_request_and_a_wait_collects_it() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/thing",
        MockExecutor::success_response(serde_json::json!({"ok": true})),
    );
    let mut store = broker(mock);

    let handle = queue(&mut store, HttpRequest::get("https://api.test/thing"));
    assert_eq!(handle.to_string(), "outstanding/0");

    // The status snapshot is always readable.
    assert!(store.read(&handle).unwrap().is_some());

    let record = store
        .read(&handle.join(&path!("response/wait")))
        .unwrap()
        .unwrap();
    let response: HttpResponse = from_value(value_of(record)).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, serde_json::json!({"ok": true}));
}

#[test]
fn unknown_handles_read_as_absent() {
    let mut store = broker(MockExecutor::new());
    assert!(store.read(&path!("outstanding/999")).unwrap().is_none());
}

#[test]
fn unknown_sub_paths_are_argument_errors() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("https://api.test/thing"));

    let error = store.read(&path!("outstanding/0/unknown")).unwrap_err();
    assert!(error.to_string().contains("Unknown sub-path"));
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
}

#[test]
fn the_recorded_timeout_is_reported() {
    let store = broker(MockExecutor::new());
    assert_eq!(store.timeout(), Duration::from_secs(5));
}

#[test]
fn outstanding_lists_references() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/a"));
    queue(&mut store, HttpRequest::get("/b"));

    let listed = items(&mut store, &path!("outstanding"));
    assert_eq!(listed.len(), 2);
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
fn writing_null_deletes_a_handle() {
    let mut store = broker(MockExecutor::new());
    let handle = queue(&mut store, HttpRequest::get("/test"));
    assert_eq!(items(&mut store, &path!("outstanding")).len(), 1);

    store.write(&handle, Record::parsed(Value::Null)).unwrap();

    assert!(items(&mut store, &path!("outstanding")).is_empty());
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
    assert!(matches!(error, Error::Conflict { .. }));
    assert!(error.to_string().contains("overwrite"));
}

#[test]
fn writes_outside_the_store_surface_are_argument_errors() {
    let mut store = broker(MockExecutor::new());
    let error = store
        .write(
            &path!("something/else"),
            Record::parsed(to_value(&HttpRequest::get("/test")).unwrap()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("Invalid write path"));
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
}

#[test]
fn the_meta_listing_references_handle_meta() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("/test"));
    assert_eq!(items(&mut store, &path!("meta/outstanding")).len(), 1);
}

#[test]
fn handle_meta_reports_state_and_navigation_including_wait() {
    let mut store = broker(MockExecutor::new());
    queue(&mut store, HttpRequest::get("https://api.test/thing"));

    let Value::Map(meta) = value_of(store.read(&path!("meta/outstanding/0")).unwrap().unwrap())
    else {
        panic!("expected a map");
    };
    assert!(meta.contains_key("state"));
    for key in ["request", "response", "wait", "delete"] {
        assert!(
            Reference::from_value(meta.get(key).unwrap()).is_some(),
            "{key}"
        );
    }
}

#[test]
fn the_delete_action_descriptor_is_served() {
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
    let Some(Value::Map(type_info)) = action.get("type") else {
        panic!("expected type info");
    };
    assert_eq!(
        type_info.get("name"),
        Some(&Value::String("action".to_string()))
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
    let mut store = BackgroundHttpBrokerStore::with_default_timeout().unwrap();
    for bad in ["not a url", "/x", "ftp://api.test/file"] {
        let handle = store
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get(bad)).unwrap()),
            )
            .unwrap();
        let error = store
            .read(&handle.join(&path!("response/wait")))
            .unwrap_err();
        assert!(
            matches!(error, Error::InvalidArgument { .. }),
            "{bad}: {error:?}"
        );
    }
}

#[test]
fn constructing_with_the_default_executor_works() {
    assert!(BackgroundHttpBrokerStore::with_default_timeout().is_ok());
    assert!(BackgroundHttpBrokerStore::new(Duration::from_secs(5)).is_ok());
}
