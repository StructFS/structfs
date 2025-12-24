#![cfg(feature = "async")]

use std::time::Duration;

use serde::{Deserialize, Serialize};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use structfs_http::async_client::AsyncHttpClientStore;
use structfs_http::{HttpRequest, RequestState, RequestStatus};
use structfs_store::{Path, Reader, Writer};

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
struct User {
    id: u64,
    name: String,
}

#[tokio::test]
async fn test_async_request_returns_handle() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/users/123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(User {
                    id: 123,
                    name: "Alice".to_string(),
                })
                .set_delay(Duration::from_millis(50)),
        )
        .mount(&server)
        .await;

    let uri = server.uri();

    let handle_path = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::get("users/123");

        // This should return immediately with a handle path
        store.write(&Path::parse("").unwrap(), &request).unwrap()
    })
    .await
    .unwrap();

    // Handle path should be like "handles/0000000000000000"
    assert!(handle_path.components[0] == "handles");
    assert_eq!(handle_path.components.len(), 2);
}

#[tokio::test]
async fn test_query_status_while_pending() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"ok": true}))
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&server)
        .await;

    let uri = server.uri();

    let (handle_path, status) = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::get("slow");

        let handle_path = store.write(&Path::parse("").unwrap(), &request).unwrap();

        // Immediately query status - should be pending
        let status: RequestStatus = store.read_owned(&handle_path).unwrap().unwrap();

        (handle_path, status)
    })
    .await
    .unwrap();

    assert_eq!(status.state, RequestState::Pending);
    assert!(handle_path.components[0] == "handles");
}

#[tokio::test]
async fn test_await_blocks_until_complete() {
    let server = MockServer::start().await;

    let user = User {
        id: 456,
        name: "Bob".to_string(),
    };

    Mock::given(method("GET"))
        .and(path("/users/456"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(&user)
                .set_delay(Duration::from_millis(50)),
        )
        .mount(&server)
        .await;

    let uri = server.uri();
    let expected_user = user.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::get("users/456");

        let handle_path = store.write(&Path::parse("").unwrap(), &request).unwrap();

        // Block until complete
        let await_path = handle_path.join(&Path::parse("await").unwrap());
        let response_path = store.write(&await_path, &()).unwrap();

        // Response path should be handles/{id}/response
        assert!(response_path.components.contains(&"response".to_string()));

        // Read the response
        let response: structfs_http::HttpResponse =
            store.read_owned(&response_path).unwrap().unwrap();

        response.json::<User>().unwrap()
    })
    .await
    .unwrap();

    assert_eq!(result, expected_user);
}

#[tokio::test]
async fn test_read_response_after_complete() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/data"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "value": 42
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let value = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::get("data");

        let handle_path = store.write(&Path::parse("").unwrap(), &request).unwrap();

        // Wait for completion
        let await_path = handle_path.join(&Path::parse("await").unwrap());
        store.write(&await_path, &()).unwrap();

        // Now read the status
        let status: RequestStatus = store.read_owned(&handle_path).unwrap().unwrap();
        assert_eq!(status.state, RequestState::Complete);
        assert!(status.response_path.is_some());

        // Read the response
        let response_path = handle_path.join(&Path::parse("response").unwrap());
        let response: structfs_http::HttpResponse =
            store.read_owned(&response_path).unwrap().unwrap();

        response.body["value"].as_i64().unwrap()
    })
    .await
    .unwrap();

    assert_eq!(value, 42);
}

#[tokio::test]
async fn test_response_none_while_pending() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/very_slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({}))
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;

    let uri = server.uri();

    let response = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::get("very_slow");

        let handle_path = store.write(&Path::parse("").unwrap(), &request).unwrap();

        // Try to read response immediately
        let response_path = handle_path.join(&Path::parse("response").unwrap());
        store.read_owned::<structfs_http::HttpResponse>(&response_path)
    })
    .await
    .unwrap();

    // Should be None because request is still pending
    assert!(response.unwrap().is_none());
}

#[tokio::test]
async fn test_custom_headers() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/protected"))
        .and(header("Authorization", "Bearer secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access": "granted"
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let result = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri)
            .unwrap()
            .with_default_header("Authorization", "Bearer secret");

        let request = HttpRequest::get("protected");
        let handle_path = store.write(&Path::parse("").unwrap(), &request).unwrap();

        // Wait and read
        let await_path = handle_path.join(&Path::parse("await").unwrap());
        store.write(&await_path, &()).unwrap();

        let response_path = handle_path.join(&Path::parse("response").unwrap());
        let response: structfs_http::HttpResponse =
            store.read_owned(&response_path).unwrap().unwrap();

        response.body["access"].as_str().unwrap().to_string()
    })
    .await
    .unwrap();

    assert_eq!(result, "granted");
}

#[tokio::test]
async fn test_multiple_concurrent_requests() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/item/1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": 1}))
                .set_delay(Duration::from_millis(30)),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/item/2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": 2}))
                .set_delay(Duration::from_millis(20)),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/item/3"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": 3}))
                .set_delay(Duration::from_millis(10)),
        )
        .mount(&server)
        .await;

    let uri = server.uri();

    let results = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();

        // Fire off three requests
        let handle1 = store
            .write(&Path::parse("").unwrap(), &HttpRequest::get("item/1"))
            .unwrap();
        let handle2 = store
            .write(&Path::parse("").unwrap(), &HttpRequest::get("item/2"))
            .unwrap();
        let handle3 = store
            .write(&Path::parse("").unwrap(), &HttpRequest::get("item/3"))
            .unwrap();

        // Wait for all (they run concurrently)
        store
            .write(&handle1.join(&Path::parse("await").unwrap()), &())
            .unwrap();
        store
            .write(&handle2.join(&Path::parse("await").unwrap()), &())
            .unwrap();
        store
            .write(&handle3.join(&Path::parse("await").unwrap()), &())
            .unwrap();

        // Read all responses
        let r1: structfs_http::HttpResponse = store
            .read_owned(&handle1.join(&Path::parse("response").unwrap()))
            .unwrap()
            .unwrap();
        let r2: structfs_http::HttpResponse = store
            .read_owned(&handle2.join(&Path::parse("response").unwrap()))
            .unwrap()
            .unwrap();
        let r3: structfs_http::HttpResponse = store
            .read_owned(&handle3.join(&Path::parse("response").unwrap()))
            .unwrap()
            .unwrap();

        vec![
            r1.body["id"].as_i64().unwrap(),
            r2.body["id"].as_i64().unwrap(),
            r3.body["id"].as_i64().unwrap(),
        ]
    })
    .await
    .unwrap();

    assert_eq!(results, vec![1, 2, 3]);
}

#[tokio::test]
async fn test_error_response_handling() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/error"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": "Internal Server Error"
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let (status, response) = tokio::task::spawn_blocking(move || {
        let mut store = AsyncHttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::get("error");

        let handle_path = store.write(&Path::parse("").unwrap(), &request).unwrap();

        // Wait for completion
        store
            .write(&handle_path.join(&Path::parse("await").unwrap()), &())
            .unwrap();

        let status: RequestStatus = store.read_owned(&handle_path).unwrap().unwrap();

        let response: structfs_http::HttpResponse = store
            .read_owned(&handle_path.join(&Path::parse("response").unwrap()))
            .unwrap()
            .unwrap();

        (status, response)
    })
    .await
    .unwrap();

    // The request should complete (not fail), but the HTTP status is 500
    assert_eq!(status.state, RequestState::Complete);
    assert_eq!(response.status, 500);
    assert!(response.is_server_error());
}
