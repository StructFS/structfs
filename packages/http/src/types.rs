use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// HTTP method for requests.
///
/// Covers every method [`http::Method`] names as a constant. Extension
/// methods are not representable — [`Method::try_from`] rejects them rather
/// than silently substituting `GET`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
#[non_exhaustive]
pub enum Method {
    #[default]
    GET,
    POST,
    PUT,
    DELETE,
    PATCH,
    HEAD,
    OPTIONS,
    CONNECT,
    TRACE,
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
            Method::CONNECT => http::Method::CONNECT,
            Method::TRACE => http::Method::TRACE,
        }
    }
}

impl TryFrom<http::Method> for Method {
    type Error = crate::Error;

    /// Convert a [`http::Method`], rejecting extension methods.
    ///
    /// Returns [`crate::Error::InvalidMethod`] for anything outside the
    /// nine methods this enum names; silently mapping unknown verbs to
    /// `GET` would turn a typo into a different request.
    fn try_from(method: http::Method) -> Result<Self, Self::Error> {
        Ok(match method {
            http::Method::GET => Method::GET,
            http::Method::POST => Method::POST,
            http::Method::PUT => Method::PUT,
            http::Method::DELETE => Method::DELETE,
            http::Method::PATCH => Method::PATCH,
            http::Method::HEAD => Method::HEAD,
            http::Method::OPTIONS => Method::OPTIONS,
            http::Method::CONNECT => Method::CONNECT,
            http::Method::TRACE => Method::TRACE,
            other => {
                return Err(crate::Error::InvalidMethod {
                    method: other.to_string(),
                })
            }
        })
    }
}

/// A full HTTP request specification
///
/// Write this struct to an HttpStore to execute the request.
/// The response will be available at the returned path.
///
/// Deserialization is strict (`deny_unknown_fields`): a map that is *not* a
/// request — `{"name": "Bob"}`, say — fails to parse instead of quietly
/// becoming `GET ""`. Stores that accept either a request or an arbitrary
/// body (see `HttpClientStore`) rely on that to tell the two apart.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
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
    /// A request with an explicit method and path.
    ///
    /// The constructor to use from outside this crate: [`HttpRequest`] is
    /// `#[non_exhaustive]`, so struct literals and `..Default::default()`
    /// are not available to downstream callers.
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            ..Default::default()
        }
    }

    /// The request's absolute URL, validated for sending.
    ///
    /// `path` must be an absolute `http` or `https` URL by the time a
    /// request reaches an executor (`HttpClientStore` resolves relative
    /// paths against its base URL first; the brokers have no base). Errors:
    ///
    /// - [`crate::Error::UrlParse`] when `path` is not a URL at all —
    ///   `"not a url"`, or a relative `"/x"`;
    /// - [`crate::Error::InvalidUrl`] for any scheme other than http/https
    ///   (`ftp://…`, `file://…`).
    ///
    /// Both map to `InvalidArgument` at the store boundary.
    pub fn url(&self) -> Result<url::Url, crate::Error> {
        let url = url::Url::parse(&self.path)?;
        match url.scheme() {
            "http" | "https" => Ok(url),
            other => Err(crate::Error::InvalidUrl {
                message: format!(
                    "unsupported scheme '{other}' in '{}'; only http and https are allowed",
                    self.path
                ),
            }),
        }
    }

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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub struct HttpResponse {
    /// HTTP status code
    pub status: u16,

    /// Status text (e.g., "OK", "Not Found")
    pub status_text: String,

    /// Response headers.
    ///
    /// A single-valued map: a header repeated in the response
    /// (`Set-Cookie`, `Via`, …) collapses to the **last** value received,
    /// and values that are not valid UTF-8 are dropped.
    pub headers: HashMap<String, String>,

    /// Response body as JSON value
    /// Will be null if body was empty or not valid JSON
    pub body: serde_json::Value,

    /// Raw body as string (useful when body isn't JSON)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
}

impl HttpResponse {
    /// A response with the given status and the canonical reason phrase.
    ///
    /// The constructor to use from outside this crate: [`HttpResponse`] is
    /// `#[non_exhaustive]`, so struct literals are not available to
    /// downstream callers. Combine with the `with_*` builders.
    pub fn new(status: u16) -> Self {
        let status_text = http::StatusCode::from_u16(status)
            .ok()
            .and_then(|code| code.canonical_reason())
            .unwrap_or("Unknown")
            .to_string();
        Self {
            status,
            status_text,
            ..Default::default()
        }
    }

    /// Override the status text (reason phrase).
    #[must_use]
    pub fn with_status_text(mut self, status_text: impl Into<String>) -> Self {
        self.status_text = status_text.into();
        self
    }

    /// Add a response header.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    /// Replace the whole header map.
    #[must_use]
    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers;
        self
    }

    /// Set the parsed JSON body, and the raw text to its JSON rendering.
    #[must_use]
    pub fn with_json_body(mut self, body: serde_json::Value) -> Self {
        self.body_text = Some(body.to_string());
        self.body = body;
        self
    }

    /// Set the raw body text without touching the parsed body.
    #[must_use]
    pub fn with_body_text(mut self, body_text: impl Into<String>) -> Self {
        self.body_text = Some(body_text.into());
        self
    }

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
        assert_eq!(http::Method::from(Method::CONNECT), http::Method::CONNECT);
        assert_eq!(http::Method::from(Method::TRACE), http::Method::TRACE);
    }

    #[test]
    fn http_method_to_method() {
        for method in [
            http::Method::GET,
            http::Method::POST,
            http::Method::PUT,
            http::Method::DELETE,
            http::Method::PATCH,
            http::Method::HEAD,
            http::Method::OPTIONS,
            http::Method::CONNECT,
            http::Method::TRACE,
        ] {
            let converted = Method::try_from(method.clone()).unwrap();
            assert_eq!(http::Method::from(converted), method);
        }
    }

    #[test]
    fn extension_methods_are_rejected_not_mapped_to_get() {
        let extension = http::Method::from_bytes(b"PURGE").unwrap();
        let error = Method::try_from(extension).unwrap_err();
        assert!(error.to_string().contains("PURGE"), "{error}");
    }

    #[test]
    fn request_urls_must_be_absolute_http_or_https() {
        assert!(HttpRequest::get("https://api.test/x").url().is_ok());
        assert!(HttpRequest::get("http://api.test/x").url().is_ok());

        for bad in ["not a url", "/x"] {
            let error = HttpRequest::get(bad).url().unwrap_err();
            assert!(matches!(error, crate::Error::UrlParse(_)), "{bad}: {error}");
        }
        for bad in ["ftp://api.test/f", "file:///etc/passwd"] {
            let error = HttpRequest::get(bad).url().unwrap_err();
            assert!(
                matches!(error, crate::Error::InvalidUrl { .. }),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn http_request_new_sets_method_and_path() {
        let req = HttpRequest::new(Method::PATCH, "/items/1");
        assert_eq!(req.method, Method::PATCH);
        assert_eq!(req.path, "/items/1");
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
        let resp = HttpResponse::new(200).with_json_body(serde_json::json!("not an object"));
        assert!(resp.json::<HashMap<String, String>>().is_err());
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

    #[test]
    fn http_request_rejects_unknown_fields() {
        // The bug this guards: with every field defaulted and unknown keys
        // accepted, *any* map parsed as a request and `{"name": "Bob"}`
        // silently became `GET ""`.
        let err = serde_json::from_str::<HttpRequest>(r#"{"name":"Bob"}"#).unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");

        let request: HttpRequest = serde_json::from_str(r#"{"method":"GET","path":"/x"}"#).unwrap();
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.path, "/x");
    }

    #[test]
    fn http_response_new_uses_canonical_reason() {
        let ok = HttpResponse::new(200);
        assert_eq!(ok.status, 200);
        assert_eq!(ok.status_text, "OK");
        assert!(ok.body.is_null());
        assert!(ok.body_text.is_none());

        assert_eq!(HttpResponse::new(299).status_text, "Unknown");
    }

    #[test]
    fn http_response_builders() {
        let response = HttpResponse::new(201)
            .with_status_text("Created")
            .with_header("Location", "/users/1")
            .with_json_body(serde_json::json!({"id": 1}));

        assert_eq!(response.status_text, "Created");
        assert_eq!(
            response.headers.get("Location"),
            Some(&"/users/1".to_string())
        );
        assert_eq!(response.body, serde_json::json!({"id": 1}));
        assert_eq!(response.body_text.as_deref(), Some(r#"{"id":1}"#));

        let text_only = HttpResponse::new(200).with_body_text("plain");
        assert!(text_only.body.is_null());
        assert_eq!(text_only.body_text.as_deref(), Some("plain"));
    }
}
