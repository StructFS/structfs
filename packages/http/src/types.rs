use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// HTTP method for requests
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    HEAD,
    OPTIONS,
}

impl From<Method> for http::Method {
    fn from(method: Method) -> Self {
        match method {
            Method::GET => http::Method::GET,
            Method::POST => http::Method::POST,
            Method::PUT => http::Method::PUT,
            Method::DELETE => http::Method::DELETE,
            Method::PATCH => http::Method::PATCH,
            Method::HEAD => http::Method::HEAD,
            Method::OPTIONS => http::Method::OPTIONS,
        }
    }
}

impl From<http::Method> for Method {
    fn from(method: http::Method) -> Self {
        match method {
            http::Method::GET => Method::GET,
            http::Method::POST => Method::POST,
            http::Method::PUT => Method::PUT,
            http::Method::DELETE => Method::DELETE,
            http::Method::PATCH => Method::PATCH,
            http::Method::HEAD => Method::HEAD,
            http::Method::OPTIONS => Method::OPTIONS,
            _ => Method::GET, // Default fallback
        }
    }
}

/// A full HTTP request specification
///
/// Write this struct to an HttpStore to execute the request.
/// The response will be available at the returned path.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpRequest {
    /// HTTP method (GET, POST, PUT, DELETE, etc.)
    #[serde(default)]
    pub method: Method,

    /// URL path (appended to base URL if using HttpClientStore)
    /// Can be a full URL if using standalone
    #[serde(default)]
    pub path: String,

    /// Query parameters
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub query: HashMap<String, String>,

    /// Request headers
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,

    /// Request body (will be JSON-serialized)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

impl HttpRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::GET,
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self {
            method: Method::POST,
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn put(path: impl Into<String>) -> Self {
        Self {
            method: Method::PUT,
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self {
            method: Method::DELETE,
            path: path.into(),
            ..Default::default()
        }
    }

    pub fn with_body(mut self, body: impl Serialize) -> Result<Self, serde_json::Error> {
        self.body = Some(serde_json::to_value(body)?);
        Ok(self)
    }

    pub fn with_json_body(mut self, body: serde_json::Value) -> Self {
        self.body = Some(body);
        self
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn with_query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.insert(name.into(), value.into());
        self
    }
}

/// HTTP response from a request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code
    pub status: u16,

    /// Status text (e.g., "OK", "Not Found")
    pub status_text: String,

    /// Response headers
    pub headers: HashMap<String, String>,

    /// Response body as JSON value
    /// Will be null if body was empty or not valid JSON
    pub body: serde_json::Value,

    /// Raw body as string (useful when body isn't JSON)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
}

impl HttpResponse {
    /// Check if the response status indicates success (2xx)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Check if the response status indicates a client error (4xx)
    pub fn is_client_error(&self) -> bool {
        (400..500).contains(&self.status)
    }

    /// Check if the response status indicates a server error (5xx)
    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.status)
    }

    /// Try to deserialize the body into a specific type
    pub fn json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_value(self.body.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_default_is_get() {
        let method = Method::default();
        assert_eq!(method, Method::GET);
    }

    #[test]
    fn method_to_http_method() {
        assert_eq!(http::Method::from(Method::GET), http::Method::GET);
        assert_eq!(http::Method::from(Method::POST), http::Method::POST);
        assert_eq!(http::Method::from(Method::PUT), http::Method::PUT);
        assert_eq!(http::Method::from(Method::DELETE), http::Method::DELETE);
        assert_eq!(http::Method::from(Method::PATCH), http::Method::PATCH);
        assert_eq!(http::Method::from(Method::HEAD), http::Method::HEAD);
        assert_eq!(http::Method::from(Method::OPTIONS), http::Method::OPTIONS);
    }

    #[test]
    fn http_method_to_method() {
        assert_eq!(Method::from(http::Method::GET), Method::GET);
        assert_eq!(Method::from(http::Method::POST), Method::POST);
        assert_eq!(Method::from(http::Method::PUT), Method::PUT);
        assert_eq!(Method::from(http::Method::DELETE), Method::DELETE);
        assert_eq!(Method::from(http::Method::PATCH), Method::PATCH);
        assert_eq!(Method::from(http::Method::HEAD), Method::HEAD);
        assert_eq!(Method::from(http::Method::OPTIONS), Method::OPTIONS);
        // Unknown methods fall back to GET
        assert_eq!(Method::from(http::Method::CONNECT), Method::GET);
    }

    #[test]
    fn http_request_get() {
        let req = HttpRequest::get("/users");
        assert_eq!(req.method, Method::GET);
        assert_eq!(req.path, "/users");
    }

    #[test]
    fn http_request_post() {
        let req = HttpRequest::post("/users");
        assert_eq!(req.method, Method::POST);
        assert_eq!(req.path, "/users");
    }

    #[test]
    fn http_request_put() {
        let req = HttpRequest::put("/users/1");
        assert_eq!(req.method, Method::PUT);
        assert_eq!(req.path, "/users/1");
    }

    #[test]
    fn http_request_delete() {
        let req = HttpRequest::delete("/users/1");
        assert_eq!(req.method, Method::DELETE);
        assert_eq!(req.path, "/users/1");
    }

    #[test]
    fn http_request_with_body() {
        #[derive(Serialize)]
        struct User {
            name: String,
        }
        let req = HttpRequest::post("/users")
            .with_body(User {
                name: "Alice".to_string(),
            })
            .unwrap();
        assert_eq!(req.body, Some(serde_json::json!({"name": "Alice"})));
    }

    #[test]
    fn http_request_with_json_body() {
        let req = HttpRequest::post("/data").with_json_body(serde_json::json!({"key": "value"}));
        assert_eq!(req.body, Some(serde_json::json!({"key": "value"})));
    }

    #[test]
    fn http_request_with_header() {
        let req = HttpRequest::get("/api").with_header("Authorization", "Bearer token123");
        assert_eq!(
            req.headers.get("Authorization"),
            Some(&"Bearer token123".to_string())
        );
    }

    #[test]
    fn http_request_with_query() {
        let req = HttpRequest::get("/search")
            .with_query("q", "test")
            .with_query("page", "1");
        assert_eq!(req.query.get("q"), Some(&"test".to_string()));
        assert_eq!(req.query.get("page"), Some(&"1".to_string()));
    }

    #[test]
    fn http_request_default() {
        let req = HttpRequest::default();
        assert_eq!(req.method, Method::GET);
        assert_eq!(req.path, "");
        assert!(req.query.is_empty());
        assert!(req.headers.is_empty());
        assert!(req.body.is_none());
    }

    #[test]
    fn http_response_is_success() {
        let resp = HttpResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: serde_json::Value::Null,
            body_text: None,
        };
        assert!(resp.is_success());
        assert!(!resp.is_client_error());
        assert!(!resp.is_server_error());
    }

    #[test]
    fn http_response_is_client_error() {
        let resp = HttpResponse {
            status: 404,
            status_text: "Not Found".to_string(),
            headers: HashMap::new(),
            body: serde_json::Value::Null,
            body_text: None,
        };
        assert!(!resp.is_success());
        assert!(resp.is_client_error());
        assert!(!resp.is_server_error());
    }

    #[test]
    fn http_response_is_server_error() {
        let resp = HttpResponse {
            status: 500,
            status_text: "Internal Server Error".to_string(),
            headers: HashMap::new(),
            body: serde_json::Value::Null,
            body_text: None,
        };
        assert!(!resp.is_success());
        assert!(!resp.is_client_error());
        assert!(resp.is_server_error());
    }

    #[test]
    fn http_response_json() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct User {
            name: String,
            age: u32,
        }
        let resp = HttpResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: serde_json::json!({"name": "Alice", "age": 30}),
            body_text: None,
        };
        let user: User = resp.json().unwrap();
        assert_eq!(
            user,
            User {
                name: "Alice".to_string(),
                age: 30
            }
        );
    }

    #[test]
    fn http_response_json_error() {
        let resp = HttpResponse {
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            body: serde_json::json!("not an object"),
            body_text: None,
        };
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Data {
            field: String,
        }
        assert!(resp.json::<Data>().is_err());
    }

    #[test]
    fn method_serde_roundtrip() {
        let method = Method::POST;
        let json = serde_json::to_string(&method).unwrap();
        assert_eq!(json, "\"POST\"");
        let parsed: Method = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Method::POST);
    }

    #[test]
    fn http_request_serde_roundtrip() {
        let req = HttpRequest::post("/api")
            .with_header("Content-Type", "application/json")
            .with_query("version", "2")
            .with_json_body(serde_json::json!({"data": 123}));

        let json = serde_json::to_string(&req).unwrap();
        let parsed: HttpRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.method, Method::POST);
        assert_eq!(parsed.path, "/api");
        assert_eq!(
            parsed.headers.get("Content-Type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(parsed.query.get("version"), Some(&"2".to_string()));
        assert_eq!(parsed.body, Some(serde_json::json!({"data": 123})));
    }
}
