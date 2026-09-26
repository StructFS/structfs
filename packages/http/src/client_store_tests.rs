//! Tests for the direct HTTP client store. Every request goes through
//! `MockExecutor`; nothing here touches the network.

use super::*;
use crate::executor::mock::MockExecutor;
use structfs_core_store::path;

fn client(mock: MockExecutor) -> HttpClientStore<MockExecutor> {
    HttpClientStore::with_executor("https://api.test", mock).unwrap()
}

#[test]
fn reading_a_path_issues_a_get() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/users/1",
        MockExecutor::success_response(serde_json::json!({"id": 1, "name": "Alice"})),
    );
    let mut store = client(mock);

    let record = store.read(&path!("users/1")).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();
    let expected = to_value(&serde_json::json!({"id": 1, "name": "Alice"})).unwrap();
    assert_eq!(value, expected);
}

#[test]
fn a_404_reads_as_absent() {
    let mock =
        MockExecutor::new().with_response("https://api.test/missing", MockExecutor::not_found());
    let mut store = client(mock);
    assert!(store.read(&path!("missing")).unwrap().is_none());
}

#[test]
fn a_server_error_is_reported_with_its_status() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/error",
        MockExecutor::error_response(500, "Internal Server Error"),
    );
    let mut store = client(mock);

    let error = store.read(&path!("error")).unwrap_err();
    assert!(error.to_string().contains("500"), "{error}");
}

#[test]
fn a_403_becomes_permission_denied() {
    let mock = MockExecutor::new().with_response(
        "https://api.test/secret",
        MockExecutor::error_response(403, "Forbidden"),
    );
    let mut store = client(mock);

    let error = store.read(&path!("secret")).unwrap_err();
    assert!(matches!(error, Error::PermissionDenied { .. }), "{error:?}");
}

#[test]
fn writing_to_a_path_posts_the_value() {
    let mock = MockExecutor::new().with_default_response(MockExecutor::success_response(
        serde_json::json!({"created": true}),
    ));
    let mut store = client(mock.clone());

    store
        .write(
            &path!("users"),
            Record::parsed(to_value(&serde_json::json!({"name": "Bob"})).unwrap()),
        )
        .unwrap();

    let requests = mock.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::POST);
    assert_eq!(requests[0].path, "https://api.test/users");
    assert_eq!(requests[0].body, Some(serde_json::json!({"name": "Bob"})));
}

#[test]
fn a_root_write_of_a_request_executes_that_request() {
    let mock = MockExecutor::new()
        .with_default_response(MockExecutor::success_response(serde_json::Value::Null));
    let mut store = client(mock.clone());

    store
        .write(
            &path!(""),
            Record::parsed(to_value(&HttpRequest::new(Method::GET, "/x")).unwrap()),
        )
        .unwrap();

    let requests = mock.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(requests[0].path, "https://api.test/x");
    assert_eq!(requests[0].body, None);
}

#[test]
fn a_root_write_of_a_plain_value_posts_it_as_the_body() {
    // The branch that used to be unreachable: `HttpRequest` accepted any
    // map, so `write / {"name": "Bob"}` silently sent `GET <base>`.
    let mock = MockExecutor::new()
        .with_default_response(MockExecutor::success_response(serde_json::Value::Null));
    let mut store = client(mock.clone());

    store
        .write(
            &path!(""),
            Record::parsed(to_value(&serde_json::json!({"name": "Bob"})).unwrap()),
        )
        .unwrap();

    let requests = mock.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::POST);
    assert_eq!(requests[0].path, "https://api.test/");
    assert_eq!(requests[0].body, Some(serde_json::json!({"name": "Bob"})));
}

fn root_write(json: serde_json::Value) -> (Result<Path, Error>, Vec<HttpRequest>) {
    let mock = MockExecutor::new()
        .with_default_response(MockExecutor::success_response(serde_json::Value::Null));
    let mut store = client(mock.clone());
    let result = store.write(&path!(""), Record::parsed(to_value(&json).unwrap()));
    (result, mock.recorded_requests())
}

#[test]
fn a_malformed_request_spec_is_rejected_not_posted() {
    for spec in [
        serde_json::json!({"method": "get", "path": "/x"}),
        serde_json::json!({"method": "PURGE", "path": "/x"}),
        serde_json::json!({"method": "GET", "path": 42}),
        serde_json::json!({"path": "/x", "headers": "not a map"}),
        serde_json::json!({"method": "GET", "path": "/x", "extra": 1}),
    ] {
        let (result, sent) = root_write(spec.clone());
        let error = result.unwrap_err();
        assert!(
            matches!(error, Error::InvalidArgument { .. }),
            "{spec}: {error:?}"
        );
        assert!(sent.is_empty(), "{spec} must not reach the network");
    }
}

#[test]
fn an_empty_map_is_posted_as_an_empty_body() {
    let (result, sent) = root_write(serde_json::json!({}));
    result.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, Method::POST);
    assert_eq!(sent[0].body, Some(serde_json::json!({})));
}

#[test]
fn a_path_only_spec_is_a_get() {
    let (result, sent) = root_write(serde_json::json!({"path": "x"}));
    result.unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, Method::GET);
    assert_eq!(sent[0].path, "https://api.test/x");
    assert_eq!(sent[0].body, None);
}

#[test]
fn a_root_write_of_a_delete_request_executes_it() {
    let mock = MockExecutor::new()
        .with_default_response(MockExecutor::success_response(serde_json::Value::Null));
    let mut store = client(mock.clone());

    store
        .write(
            &path!(""),
            Record::parsed(to_value(&HttpRequest::delete("/users/1")).unwrap()),
        )
        .unwrap();

    let requests = mock.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::DELETE);
}

#[test]
fn default_headers_are_applied_and_overridable() {
    let mock = MockExecutor::new()
        .with_default_response(MockExecutor::success_response(serde_json::Value::Null));
    let mut store = client(mock.clone()).with_default_header("Authorization", "Bearer token123");

    store.read(&path!("data")).unwrap();
    assert_eq!(
        mock.recorded_requests()[0].headers.get("Authorization"),
        Some(&"Bearer token123".to_string())
    );

    let store = client(mock.clone()).with_default_header("X-Custom", "default");
    let built = store.build_request(HttpRequest::get("/data").with_header("X-Custom", "override"));
    assert_eq!(built.headers.get("X-Custom"), Some(&"override".to_string()));
}

#[test]
fn relative_paths_resolve_against_the_base_url_and_absolute_ones_do_not() {
    let mock = MockExecutor::new()
        .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

    let store = HttpClientStore::with_executor("https://api.test/v1/", mock.clone()).unwrap();
    assert_eq!(
        store.build_request(HttpRequest::get("users")).path,
        "https://api.test/v1/users"
    );

    let store = client(mock);
    assert_eq!(
        store
            .build_request(HttpRequest::get("https://other.test/data"))
            .path,
        "https://other.test/data"
    );
}

#[test]
fn a_failing_write_reports_its_status() {
    let mock =
        MockExecutor::new().with_default_response(MockExecutor::error_response(400, "Bad Request"));
    let mut store = client(mock);

    let error = store
        .write(
            &path!("data"),
            Record::parsed(to_value(&serde_json::json!({})).unwrap()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("400"), "{error}");
}

#[test]
fn a_404_on_write_is_a_typed_not_found_for_the_store_path() {
    let mock = MockExecutor::new().with_default_response(MockExecutor::not_found());
    let mut store = client(mock);

    let error = store
        .write(
            &path!("data"),
            Record::parsed(to_value(&serde_json::json!({})).unwrap()),
        )
        .unwrap_err();
    match error {
        Error::NotFound { path } => assert_eq!(path.to_string(), "data"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn docs_describe_the_store() {
    let mut store = client(MockExecutor::new());
    let Value::Map(docs) = store
        .read(&path!("docs"))
        .unwrap()
        .unwrap()
        .into_value(&NoCodec)
        .unwrap()
    else {
        panic!("expected a map");
    };
    for key in ["title", "description", "paths", "example"] {
        assert!(docs.contains_key(key), "missing {key}");
    }
}

#[test]
fn construction_validates_the_base_url() {
    assert!(HttpClientStore::new("https://api.test").is_ok());
    assert!(HttpClientStore::new("not a url").is_err());
}
