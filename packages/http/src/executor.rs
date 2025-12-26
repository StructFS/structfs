//! HTTP execution abstraction for testing.
//!
//! This module provides a trait for HTTP execution that can be mocked in tests,
//! avoiding the need for actual network calls.

use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::types::{HttpRequest, HttpResponse};

/// Trait for executing HTTP requests.
///
/// Implementations can use real HTTP clients or mock responses for testing.
pub trait HttpExecutor: Send + Sync {
    /// Execute an HTTP request and return the response.
    ///
    /// Returns `Err` with a message if the request fails.
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
}

/// Production HTTP executor using reqwest.
pub struct ReqwestExecutor {
    client: Client,
}

impl ReqwestExecutor {
    /// Create a new executor with the given timeout.
    pub fn new(timeout: Duration) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| e.to_string())?;

        Ok(Self { client })
    }

    /// Create with default timeout of 30 seconds.
    pub fn with_default_timeout() -> Result<Self, String> {
        Self::new(Duration::from_secs(30))
    }
}

impl HttpExecutor for ReqwestExecutor {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let method: http::Method = request.method.clone().into();

        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let header_name = HeaderName::try_from(name.as_str()).map_err(|e| e.to_string())?;
            let header_value = HeaderValue::try_from(value.as_str()).map_err(|e| e.to_string())?;
            headers.insert(header_name, header_value);
        }

        let mut req_builder = self.client.request(method, &request.path);
        req_builder = req_builder.headers(headers);

        if !request.query.is_empty() {
            req_builder = req_builder.query(&request.query);
        }

        if let Some(body) = &request.body {
            req_builder = req_builder.json(body);
        }

        let response = req_builder.send().map_err(|e| e.to_string())?;

        let status = response.status().as_u16();
        let status_text = response
            .status()
            .canonical_reason()
            .unwrap_or("Unknown")
            .to_string();

        let mut resp_headers = std::collections::HashMap::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                resp_headers.insert(name.to_string(), v.to_string());
            }
        }

        let body_text = response.text().map_err(|e| e.to_string())?;
        let body = serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);

        Ok(HttpResponse {
            status,
            status_text,
            headers: resp_headers,
            body,
            body_text: Some(body_text),
        })
    }
}

/// Mock HTTP executor for testing.
///
/// Returns predefined responses based on request matching.
#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A mock HTTP executor that returns predefined responses.
    #[derive(Clone, Default)]
    pub struct MockExecutor {
        /// Responses keyed by request path.
        responses: Arc<Mutex<HashMap<String, HttpResponse>>>,
        /// Default response when no match found.
        default_response: Arc<Mutex<Option<HttpResponse>>>,
        /// Recorded requests for verification.
        recorded_requests: Arc<Mutex<Vec<HttpRequest>>>,
        /// Whether to fail all requests.
        fail_all: Arc<Mutex<bool>>,
        /// Custom error message when failing.
        error_message: Arc<Mutex<Option<String>>>,
    }

    impl MockExecutor {
        /// Create a new mock executor.
        pub fn new() -> Self {
            Self::default()
        }

        /// Add a response for a specific path.
        pub fn with_response(self, path: impl Into<String>, response: HttpResponse) -> Self {
            self.responses.lock().unwrap().insert(path.into(), response);
            self
        }

        /// Set a default response when no path matches.
        pub fn with_default_response(self, response: HttpResponse) -> Self {
            *self.default_response.lock().unwrap() = Some(response);
            self
        }

        /// Configure to fail all requests with an error.
        pub fn fail_with(self, message: impl Into<String>) -> Self {
            *self.fail_all.lock().unwrap() = true;
            *self.error_message.lock().unwrap() = Some(message.into());
            self
        }

        /// Get all recorded requests.
        pub fn recorded_requests(&self) -> Vec<HttpRequest> {
            self.recorded_requests.lock().unwrap().clone()
        }

        /// Clear recorded requests.
        pub fn clear_recorded(&self) {
            self.recorded_requests.lock().unwrap().clear();
        }

        /// Create a simple success response.
        pub fn success_response(body: serde_json::Value) -> HttpResponse {
            let body_text = body.to_string();
            HttpResponse {
                status: 200,
                status_text: "OK".to_string(),
                headers: HashMap::new(),
                body,
                body_text: Some(body_text),
            }
        }

        /// Create a simple error response.
        pub fn error_response(status: u16, message: &str) -> HttpResponse {
            HttpResponse {
                status,
                status_text: message.to_string(),
                headers: HashMap::new(),
                body: serde_json::json!({"error": message}),
                body_text: Some(format!(r#"{{"error":"{}"}}"#, message)),
            }
        }

        /// Create a 404 Not Found response.
        pub fn not_found() -> HttpResponse {
            Self::error_response(404, "Not Found")
        }
    }

    impl HttpExecutor for MockExecutor {
        fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            // Record the request
            self.recorded_requests.lock().unwrap().push(request.clone());

            // Check if we should fail
            if *self.fail_all.lock().unwrap() {
                let msg = self
                    .error_message
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "Mock failure".to_string());
                return Err(msg);
            }

            // Look for a matching response
            let responses = self.responses.lock().unwrap();
            if let Some(response) = responses.get(&request.path) {
                return Ok(response.clone());
            }

            // Use default response if available
            if let Some(ref response) = *self.default_response.lock().unwrap() {
                return Ok(response.clone());
            }

            // No match - return 404
            Ok(Self::not_found())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockExecutor;
    use super::*;
    use crate::types::Method;
    use std::collections::HashMap;

    #[test]
    fn mock_executor_returns_configured_response() {
        let response = HttpResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: serde_json::json!({"result": "success"}),
            body_text: Some(r#"{"result":"success"}"#.to_string()),
        };

        let executor = MockExecutor::new().with_response("/test", response.clone());

        let request = HttpRequest::get("/test");
        let result = executor.execute(&request).unwrap();

        assert_eq!(result.status, 200);
        assert_eq!(result.body, serde_json::json!({"result": "success"}));
    }

    #[test]
    fn mock_executor_returns_default_response() {
        let default = MockExecutor::success_response(serde_json::json!({"default": true}));
        let executor = MockExecutor::new().with_default_response(default);

        let request = HttpRequest::get("/any-path");
        let result = executor.execute(&request).unwrap();

        assert_eq!(result.status, 200);
        assert_eq!(result.body, serde_json::json!({"default": true}));
    }

    #[test]
    fn mock_executor_returns_404_when_no_match() {
        let executor = MockExecutor::new();
        let request = HttpRequest::get("/unknown");
        let result = executor.execute(&request).unwrap();

        assert_eq!(result.status, 404);
    }

    #[test]
    fn mock_executor_fails_when_configured() {
        let executor = MockExecutor::new().fail_with("Network error");
        let request = HttpRequest::get("/any");
        let result = executor.execute(&request);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Network error");
    }

    #[test]
    fn mock_executor_records_requests() {
        let executor = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        executor.execute(&HttpRequest::get("/first")).unwrap();
        executor.execute(&HttpRequest::post("/second")).unwrap();
        executor.execute(&HttpRequest::delete("/third")).unwrap();

        let recorded = executor.recorded_requests();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].path, "/first");
        assert_eq!(recorded[0].method, Method::GET);
        assert_eq!(recorded[1].path, "/second");
        assert_eq!(recorded[1].method, Method::POST);
        assert_eq!(recorded[2].path, "/third");
        assert_eq!(recorded[2].method, Method::DELETE);
    }

    #[test]
    fn mock_executor_clear_recorded() {
        let executor = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        executor.execute(&HttpRequest::get("/test")).unwrap();
        assert_eq!(executor.recorded_requests().len(), 1);

        executor.clear_recorded();
        assert!(executor.recorded_requests().is_empty());
    }

    #[test]
    fn mock_executor_success_response_helper() {
        let response = MockExecutor::success_response(serde_json::json!({"key": "value"}));
        assert_eq!(response.status, 200);
        assert_eq!(response.status_text, "OK");
        assert_eq!(response.body, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn mock_executor_error_response_helper() {
        let response = MockExecutor::error_response(500, "Internal Error");
        assert_eq!(response.status, 500);
        assert_eq!(response.status_text, "Internal Error");
    }

    #[test]
    fn mock_executor_not_found_helper() {
        let response = MockExecutor::not_found();
        assert_eq!(response.status, 404);
    }

    #[test]
    fn mock_executor_with_headers_in_request() {
        let executor = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let request = HttpRequest::get("/api").with_header("Authorization", "Bearer token");

        executor.execute(&request).unwrap();

        let recorded = executor.recorded_requests();
        assert_eq!(
            recorded[0].headers.get("Authorization"),
            Some(&"Bearer token".to_string())
        );
    }

    #[test]
    fn mock_executor_with_query_params() {
        let executor = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let request = HttpRequest::get("/search")
            .with_query("q", "test")
            .with_query("page", "1");

        executor.execute(&request).unwrap();

        let recorded = executor.recorded_requests();
        assert_eq!(recorded[0].query.get("q"), Some(&"test".to_string()));
        assert_eq!(recorded[0].query.get("page"), Some(&"1".to_string()));
    }

    #[test]
    fn mock_executor_with_body() {
        let executor = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let request =
            HttpRequest::post("/data").with_json_body(serde_json::json!({"name": "test"}));

        executor.execute(&request).unwrap();

        let recorded = executor.recorded_requests();
        assert_eq!(recorded[0].body, Some(serde_json::json!({"name": "test"})));
    }

    #[test]
    fn mock_executor_multiple_responses() {
        let executor = MockExecutor::new()
            .with_response(
                "/users",
                MockExecutor::success_response(serde_json::json!({"users": []})),
            )
            .with_response(
                "/posts",
                MockExecutor::success_response(serde_json::json!({"posts": []})),
            );

        let users = executor.execute(&HttpRequest::get("/users")).unwrap();
        let posts = executor.execute(&HttpRequest::get("/posts")).unwrap();

        assert_eq!(users.body, serde_json::json!({"users": []}));
        assert_eq!(posts.body, serde_json::json!({"posts": []}));
    }

    #[test]
    fn reqwest_executor_creation() {
        let executor = ReqwestExecutor::with_default_timeout();
        assert!(executor.is_ok());
    }

    #[test]
    fn reqwest_executor_custom_timeout() {
        let executor = ReqwestExecutor::new(Duration::from_secs(10));
        assert!(executor.is_ok());
    }
}
