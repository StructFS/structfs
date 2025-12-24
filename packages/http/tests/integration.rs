use serde::{Deserialize, Serialize};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use structfs_http::blocking::HttpClientStore;
use structfs_http::HttpRequest;
use structfs_store::{Path, Reader, Writer};

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
struct User {
    id: u64,
    name: String,
    email: String,
}

#[tokio::test]
async fn test_get_request_via_read() {
    let server = MockServer::start().await;

    let user = User {
        id: 123,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    };

    Mock::given(method("GET"))
        .and(path("/users/123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&user))
        .mount(&server)
        .await;

    let uri = server.uri();
    let expected_user = user.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri).unwrap();
        client
            .read_owned::<User>(&Path::parse("users/123").unwrap())
            .unwrap()
            .unwrap()
    })
    .await
    .unwrap();

    assert_eq!(result, expected_user);
}

#[tokio::test]
async fn test_get_returns_none_on_404() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/users/999"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
            "error": "Not found"
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let result = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri).unwrap();
        client
            .read_owned::<User>(&Path::parse("users/999").unwrap())
            .unwrap()
    })
    .await
    .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn test_post_request_via_write() {
    let server = MockServer::start().await;

    let new_user = User {
        id: 0,
        name: "Bob".to_string(),
        email: "bob@example.com".to_string(),
    };

    let created_user = User {
        id: 456,
        name: "Bob".to_string(),
        email: "bob@example.com".to_string(),
    };

    Mock::given(method("POST"))
        .and(path("/users"))
        .and(body_json(&new_user))
        .respond_with(ResponseTemplate::new(201).set_body_json(&created_user))
        .mount(&server)
        .await;

    let uri = server.uri();
    let user_to_send = new_user.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri).unwrap();
        client.write(&Path::parse("users").unwrap(), &user_to_send)
    })
    .await
    .unwrap();

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_custom_request_with_put() {
    let server = MockServer::start().await;

    let updated_user = User {
        id: 123,
        name: "Alice Updated".to_string(),
        email: "alice.new@example.com".to_string(),
    };

    Mock::given(method("PUT"))
        .and(path("/users/123"))
        .and(header("Authorization", "Bearer token123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&updated_user))
        .mount(&server)
        .await;

    let uri = server.uri();
    let user_to_send = updated_user.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri).unwrap();

        let request = HttpRequest::put("users/123")
            .with_header("Authorization", "Bearer token123")
            .with_body(&user_to_send)
            .unwrap();

        client.write(&Path::parse("").unwrap(), &request)
    })
    .await
    .unwrap();

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_delete_request() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/users/123"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let uri = server.uri();

    let result = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri).unwrap();
        let request = HttpRequest::delete("users/123");
        client.write(&Path::parse("").unwrap(), &request)
    })
    .await
    .unwrap();

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_default_headers() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/protected"))
        .and(header("Authorization", "Bearer default-token"))
        .and(header("X-Api-Key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "ok"
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let result = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri)
            .unwrap()
            .with_default_header("Authorization", "Bearer default-token")
            .with_default_header("X-Api-Key", "secret");

        client
            .read_owned::<serde_json::Value>(&Path::parse("protected").unwrap())
            .unwrap()
            .unwrap()
    })
    .await
    .unwrap();

    assert_eq!(result["status"], "ok");
}

#[tokio::test]
async fn test_query_parameters() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/search"))
        .and(wiremock::matchers::query_param("q", "rust"))
        .and(wiremock::matchers::query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": ["result1", "result2"]
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let response = tokio::task::spawn_blocking(move || {
        let client = HttpClientStore::new(&uri).unwrap();

        let request = HttpRequest::get("search")
            .with_query("q", "rust")
            .with_query("limit", "10");

        client.request(&request).unwrap()
    })
    .await
    .unwrap();

    assert!(response.is_success());
}

#[tokio::test]
async fn test_http_response_fields() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/data"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Custom-Header", "custom-value")
                .insert_header("Content-Type", "application/json")
                .set_body_json(serde_json::json!({
                    "message": "Hello, World!"
                })),
        )
        .mount(&server)
        .await;

    let uri = server.uri();

    let response = tokio::task::spawn_blocking(move || {
        let client = HttpClientStore::new(&uri).unwrap();
        client.get(&Path::parse("api/data").unwrap()).unwrap()
    })
    .await
    .unwrap();

    assert_eq!(response.status, 200);
    assert_eq!(response.status_text, "OK");
    assert!(response.is_success());
    assert!(!response.is_client_error());
    assert!(!response.is_server_error());
    assert_eq!(
        response.headers.get("x-custom-header"),
        Some(&"custom-value".to_string())
    );
    assert_eq!(response.body["message"], "Hello, World!");
}

#[tokio::test]
async fn test_error_response() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/error"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "error": "Internal Server Error"
        })))
        .mount(&server)
        .await;

    let uri = server.uri();

    let (read_result, get_response) = tokio::task::spawn_blocking(move || {
        let mut client = HttpClientStore::new(&uri).unwrap();

        // read_owned should return an error for 5xx
        let read_result: Result<Option<serde_json::Value>, _> =
            client.read_owned(&Path::parse("api/error").unwrap());

        // Direct get() should return the response
        let get_response = client.get(&Path::parse("api/error").unwrap()).unwrap();

        (read_result, get_response)
    })
    .await
    .unwrap();

    assert!(read_result.is_err());
    assert_eq!(get_response.status, 500);
    assert!(get_response.is_server_error());
}
