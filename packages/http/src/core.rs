//! New architecture implementations of HTTP stores using core-store.
//!
//! This module provides implementations using the new three-layer architecture
//! (ll-store, core-store, serde-store) instead of the legacy erased_serde approach.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use structfs_core_store::{path, Error, NoCodec, Path, Reader, Record, Writer};
use structfs_serde_store::{from_value, to_value};

use crate::executor::{HttpExecutor, ReqwestExecutor};
use crate::handle::RequestStatus;

use crate::types::{HttpRequest, HttpResponse};

const OUTSTANDING_PREFIX: &str = "outstanding";

type RequestId = u64;

/// State of a request handle in the sync broker.
///
/// Requests transition through states: Queued -> Executed (with cached response or error).
/// The response is cached and subsequent reads return the same result (idempotent).
#[derive(Debug)]
struct SyncRequestHandle {
    request: HttpRequest,
    response: Option<HttpResponse>,
    error: Option<String>,
}

impl SyncRequestHandle {
    fn new(request: HttpRequest) -> Self {
        Self {
            request,
            response: None,
            error: None,
        }
    }

    /// Returns true if this handle has been executed (success or failure).
    fn is_executed(&self) -> bool {
        self.response.is_some() || self.error.is_some()
    }
}

/// HTTP broker store for sync (blocking) requests (new architecture).
///
/// Write requests are queued and executed when reading from the handle path.
/// **Reads are idempotent**: the first read executes the request and caches the result;
/// subsequent reads return the cached response.
///
/// ## Path Structure
///
/// | Path | Operation | Result |
/// |------|-----------|--------|
/// | `write /` | Queue request | Returns `outstanding/{id}` |
/// | `read /outstanding` | List handles | Returns `[0, 1, 2, ...]` |
/// | `read /outstanding/{id}` | Execute & return response | Returns cached response |
/// | `read /outstanding/{id}/request` | View queued request | Returns original request |
/// | `write /outstanding/{id} null` | Delete handle | Removes handle |
///
/// Generic over the HTTP executor to allow mocking in tests.
pub struct HttpBrokerStore<E: HttpExecutor = ReqwestExecutor> {
    handles: BTreeMap<RequestId, SyncRequestHandle>,
    next_request_id: RequestId,
    executor: E,
}

impl HttpBrokerStore<ReqwestExecutor> {
    /// Create a new HTTP broker store with the given request timeout.
    pub fn new(timeout: Duration) -> Result<Self, crate::Error> {
        let executor =
            ReqwestExecutor::new(timeout).map_err(|e| crate::Error::InvalidUrl { message: e })?;

        Ok(Self {
            handles: BTreeMap::new(),
            next_request_id: 0,
            executor,
        })
    }

    /// Create with default timeout of 30 seconds.
    pub fn with_default_timeout() -> Result<Self, crate::Error> {
        Self::new(Duration::from_secs(30))
    }
}

impl<E: HttpExecutor> HttpBrokerStore<E> {
    /// Create a new HTTP broker store with a custom executor.
    ///
    /// This is primarily useful for testing with mock executors.
    pub fn with_executor(executor: E) -> Self {
        Self {
            handles: BTreeMap::new(),
            next_request_id: 0,
            executor,
        }
    }

    /// Parse request ID and optional sub-path from paths like:
    /// - "outstanding" -> None (listing)
    /// - "outstanding/123" -> Some((123, None))
    /// - "outstanding/123/request" -> Some((123, Some("request")))
    fn parse_handle_path(path: &Path) -> Option<(RequestId, Option<&str>)> {
        if path.is_empty() || path[0] != OUTSTANDING_PREFIX {
            return None;
        }
        if path.len() == 1 {
            // Just "outstanding" - listing request
            return None;
        }
        let id: RequestId = path[1].parse().ok()?;
        let sub_path = if path.len() > 2 {
            Some(path[2].as_str())
        } else {
            None
        };
        Some((id, sub_path))
    }

    /// Check if a handle exists (for testing).
    #[cfg(test)]
    pub fn has_handle(&self, id: RequestId) -> bool {
        self.handles.contains_key(&id)
    }

    /// Get the number of handles (for testing).
    #[cfg(test)]
    pub fn handle_count(&self) -> usize {
        self.handles.len()
    }
}

impl<E: HttpExecutor> Reader for HttpBrokerStore<E> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Handle listing: read /outstanding -> [0, 1, 2, ...]
        if from.len() == 1 && from[0] == OUTSTANDING_PREFIX {
            let ids: Vec<structfs_core_store::Value> = self
                .handles
                .keys()
                .map(|id| structfs_core_store::Value::Integer(*id as i64))
                .collect();
            return Ok(Some(Record::parsed(structfs_core_store::Value::Array(ids))));
        }

        // Parse /outstanding/{id} or /outstanding/{id}/request
        let (request_id, sub_path) = Self::parse_handle_path(from).ok_or_else(|| {
            Error::store(
                "http_broker",
                "read",
                format!(
                    "Invalid path '{}'. Expected: outstanding, outstanding/{{id}}, or outstanding/{{id}}/request",
                    from
                ),
            )
        })?;

        let handle = self.handles.get_mut(&request_id).ok_or_else(|| {
            Error::store(
                "http_broker",
                "read",
                format!("Request with ID {} not found", request_id),
            )
        })?;

        // Return queued request at /outstanding/{id}/request
        if sub_path == Some("request") {
            let value = to_value(&handle.request)
                .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
            return Ok(Some(Record::parsed(value)));
        }

        // Reject unknown sub-paths
        if let Some(unknown) = sub_path {
            return Err(Error::store(
                "http_broker",
                "read",
                format!(
                    "Unknown sub-path '{}'. Use 'request' to view the queued request.",
                    unknown
                ),
            ));
        }

        // Execute on first read if not yet executed (idempotent)
        if !handle.is_executed() {
            match self.executor.execute(&handle.request) {
                Ok(response) => handle.response = Some(response),
                Err(e) => handle.error = Some(e),
            }
        }

        // Return cached response or error
        if let Some(ref response) = handle.response {
            let value = to_value(response)
                .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
            Ok(Some(Record::parsed(value)))
        } else if let Some(ref error) = handle.error {
            Err(Error::store(
                "http_broker",
                "read",
                format!("HTTP request failed: {}", error),
            ))
        } else {
            unreachable!("handle.is_executed() was true but neither response nor error is set")
        }
    }
}

impl<E: HttpExecutor> Writer for HttpBrokerStore<E> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;

        // Delete handle: write null to /outstanding/{id}
        if let Some((request_id, None)) = Self::parse_handle_path(to) {
            if value == structfs_core_store::Value::Null {
                self.handles.remove(&request_id);
                return Ok(to.clone());
            }
            return Err(Error::store(
                "http_broker",
                "write",
                "Cannot overwrite existing request. Write null to delete, or write to root to queue a new request.",
            ));
        }

        // Queue new request: write to root
        if to.is_empty() {
            let request: HttpRequest = from_value(value).map_err(|e| {
                Error::decode(
                    structfs_core_store::Format::JSON,
                    format!("Data must be an HttpRequest: {}", e),
                )
            })?;

            let request_id = self.next_request_id;
            self.next_request_id += 1;

            self.handles
                .insert(request_id, SyncRequestHandle::new(request));

            return Ok(path!(OUTSTANDING_PREFIX).join(&path!(&format!("{}", request_id))));
        }

        Err(Error::store(
            "http_broker",
            "write",
            format!(
                "Invalid write path '{}'. Write to root to queue a request, or write null to outstanding/{{id}} to delete.",
                to
            ),
        ))
    }
}

/// HTTP client store for direct requests (new architecture).
///
/// Maps read/write operations to GET/POST requests.
/// Generic over the HTTP executor to allow mocking in tests.
pub struct HttpClientStore<E: HttpExecutor = ReqwestExecutor> {
    executor: E,
    base_url: url::Url,
    default_headers: std::collections::HashMap<String, String>,
}

impl HttpClientStore<ReqwestExecutor> {
    /// Create a new HTTP client store with the given base URL
    pub fn new(base_url: &str) -> Result<Self, crate::Error> {
        let base_url = url::Url::parse(base_url)?;
        let executor = ReqwestExecutor::with_default_timeout()
            .map_err(|e| crate::Error::InvalidUrl { message: e })?;

        Ok(Self {
            executor,
            base_url,
            default_headers: std::collections::HashMap::new(),
        })
    }
}

impl<E: HttpExecutor> HttpClientStore<E> {
    /// Create a new HTTP client store with a custom executor.
    ///
    /// This is primarily useful for testing with mock executors.
    pub fn with_executor(base_url: &str, executor: E) -> Result<Self, crate::Error> {
        let base_url = url::Url::parse(base_url)?;

        Ok(Self {
            executor,
            base_url,
            default_headers: std::collections::HashMap::new(),
        })
    }

    /// Add a default header that will be sent with every request
    pub fn with_default_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.default_headers.insert(name.into(), value.into());
        self
    }

    /// Build a full request with base URL, default headers, etc.
    fn build_request(&self, mut request: HttpRequest) -> HttpRequest {
        // Resolve relative URLs against base URL
        if !request.path.starts_with("http://") && !request.path.starts_with("https://") {
            if let Ok(url) = self.base_url.join(&request.path) {
                request.path = url.to_string();
            }
        }

        // Add default headers (request headers take precedence)
        for (name, value) in &self.default_headers {
            if !request.headers.contains_key(name) {
                request.headers.insert(name.clone(), value.clone());
            }
        }

        request
    }

    /// Perform a GET request and return the response
    pub fn get(&self, path: &Path) -> Result<HttpResponse, crate::Error> {
        let request = HttpRequest {
            method: crate::types::Method::GET,
            path: path.components.join("/"),
            ..Default::default()
        };
        let full_request = self.build_request(request);
        self.executor
            .execute(&full_request)
            .map_err(|e| crate::Error::Other {
                message: format!("HTTP request failed: {}", e),
            })
    }
}

impl<E: HttpExecutor> Reader for HttpClientStore<E> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let response = self
            .get(from)
            .map_err(|e| Error::store("http_client", "read", e.to_string()))?;

        if response.status == 404 {
            return Ok(None);
        }

        if !response.is_success() {
            return Err(Error::store(
                "http_client",
                "read",
                format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            ));
        }

        // Convert response body to Value
        let value = to_value(&response.body)
            .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;

        Ok(Some(Record::parsed(value)))
    }
}

impl<E: HttpExecutor> Writer for HttpClientStore<E> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;

        // Try to interpret as HttpRequest if writing to root
        let response = if to.is_empty() {
            if let Ok(request) = from_value::<HttpRequest>(value.clone()) {
                // It's an HttpRequest, execute it directly
                let full_request = self.build_request(request);
                self.executor
                    .execute(&full_request)
                    .map_err(|e| Error::store("http_client", "write", e))?
            } else {
                // Not an HttpRequest, POST to root with the value as body
                let json_value = structfs_serde_store::value_to_json(value);
                let request = HttpRequest {
                    method: crate::types::Method::POST,
                    path: String::new(),
                    body: Some(json_value),
                    ..Default::default()
                };
                let full_request = self.build_request(request);
                self.executor
                    .execute(&full_request)
                    .map_err(|e| Error::store("http_client", "write", e))?
            }
        } else {
            // POST to the path
            let json_value = structfs_serde_store::value_to_json(value);
            let request = HttpRequest {
                method: crate::types::Method::POST,
                path: to.components.join("/"),
                body: Some(json_value),
                ..Default::default()
            };
            let full_request = self.build_request(request);
            self.executor
                .execute(&full_request)
                .map_err(|e| Error::store("http_client", "write", e))?
        };

        if !response.is_success() {
            return Err(Error::store(
                "http_client",
                "write",
                format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            ));
        }

        Ok(to.clone())
    }
}

/// Internal state for an async request
struct AsyncRequestHandle {
    status: RequestStatus,
    response: Option<HttpResponse>,
}

/// Async HTTP broker store (new architecture).
///
/// Requests are executed in background threads. Write to queue a request,
/// read from the handle to check status or get the response.
pub struct AsyncHttpBrokerStore {
    handles: Arc<Mutex<HashMap<RequestId, AsyncRequestHandle>>>,
    next_request_id: RequestId,
    timeout: Duration,
}

impl AsyncHttpBrokerStore {
    /// Create a new async HTTP broker store with the given request timeout.
    pub fn new(timeout: Duration) -> Result<Self, crate::Error> {
        Ok(Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: 0,
            timeout,
        })
    }

    /// Create with default timeout of 30 seconds.
    pub fn with_default_timeout() -> Result<Self, crate::Error> {
        Self::new(Duration::from_secs(30))
    }

    /// Execute an HTTP request and return the response.
    fn execute_request(request: HttpRequest, timeout: Duration) -> Result<HttpResponse, String> {
        let client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| e.to_string())?;

        let method: http::Method = request.method.into();

        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let header_name = HeaderName::try_from(name.as_str()).map_err(|e| e.to_string())?;
            let header_value = HeaderValue::try_from(value.as_str()).map_err(|e| e.to_string())?;
            headers.insert(header_name, header_value);
        }

        let mut req_builder = client.request(method, &request.path);
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

    /// Parse request ID and sub-path from a path like "outstanding/123" or "outstanding/123/response".
    fn parse_handle_path(path: &Path) -> Option<(RequestId, Option<String>)> {
        if path.is_empty() || path[0] != OUTSTANDING_PREFIX {
            return None;
        }
        if path.len() < 2 {
            return None;
        }
        let id: RequestId = path[1].parse().ok()?;
        let sub_path = if path.len() > 2 {
            Some(path.components[2..].join("/"))
        } else {
            None
        };
        Some((id, sub_path))
    }
}

impl Reader for AsyncHttpBrokerStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let (request_id, sub_path) = Self::parse_handle_path(from).ok_or_else(|| {
            Error::store(
                "async_http_broker",
                "read",
                format!(
                    "Invalid handle path '{}'. Expected format: outstanding/{{id}}[/response]",
                    from
                ),
            )
        })?;

        let handles = self
            .handles
            .lock()
            .map_err(|e| Error::store("async_http_broker", "read", format!("Lock error: {}", e)))?;

        let handle = handles.get(&request_id).ok_or_else(|| {
            Error::store(
                "async_http_broker",
                "read",
                format!("Request with ID {} not found", request_id),
            )
        })?;

        let value = match sub_path.as_deref() {
            Some("response") => {
                if let Some(ref response) = handle.response {
                    to_value(response).map_err(|e| {
                        Error::encode(structfs_core_store::Format::JSON, e.to_string())
                    })?
                } else {
                    return Ok(None); // Response not ready yet
                }
            }
            None => to_value(&handle.status)
                .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?,
            Some(other) => {
                return Err(Error::store(
                    "async_http_broker",
                    "read",
                    format!(
                        "Unknown sub-path '{}'. Use 'response' to get the response.",
                        other
                    ),
                ));
            }
        };

        Ok(Some(Record::parsed(value)))
    }
}

impl Writer for AsyncHttpBrokerStore {
    fn write(&mut self, _to: &Path, data: Record) -> Result<Path, Error> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        // Convert Record to HttpRequest
        let value = data.into_value(&NoCodec)?;
        let request: HttpRequest = from_value(value).map_err(|e| {
            Error::decode(
                structfs_core_store::Format::JSON,
                format!("Data must be an HttpRequest: {}", e),
            )
        })?;

        // Create initial pending status
        let handle = AsyncRequestHandle {
            status: RequestStatus::pending(request_id.to_string()),
            response: None,
        };

        {
            let mut handles = self.handles.lock().map_err(|e| {
                Error::store("async_http_broker", "write", format!("Lock error: {}", e))
            })?;
            handles.insert(request_id, handle);
        }

        // Spawn background thread to execute the request
        let handles = Arc::clone(&self.handles);
        let timeout = self.timeout;
        thread::spawn(move || {
            let result = Self::execute_request(request, timeout);

            if let Ok(mut handles) = handles.lock() {
                if let Some(handle) = handles.get_mut(&request_id) {
                    match result {
                        Ok(response) => {
                            handle.status = RequestStatus::complete(request_id.to_string());
                            handle.response = Some(response);
                        }
                        Err(error) => {
                            handle.status = RequestStatus::failed(request_id.to_string(), error);
                        }
                    }
                }
            }
        });

        Ok(path!(OUTSTANDING_PREFIX).join(&path!(&format!("{}", request_id))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::mock::MockExecutor;

    // ==================== HttpBrokerStore tests ====================

    #[test]
    fn test_parse_handle_path() {
        // Just "outstanding" returns None (listing)
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("outstanding")),
            None
        );
        // Basic handle path
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("outstanding/0")),
            Some((0, None))
        );
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("outstanding/123")),
            Some((123, None))
        );
        // With sub-path
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("outstanding/0/request")),
            Some((0, Some("request")))
        );
        // Invalid paths
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("other/123")),
            None
        );
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("outstanding/abc")),
            None
        );
        assert_eq!(
            HttpBrokerStore::<ReqwestExecutor>::parse_handle_path(&path!("")),
            None
        );
    }

    #[test]
    fn test_broker_queue_request() {
        let mut broker = HttpBrokerStore::with_default_timeout().unwrap();

        // Create a request
        let request_value = to_value(&HttpRequest::get("https://example.com")).unwrap();

        // Write the request
        let handle = broker
            .write(&path!(""), Record::parsed(request_value))
            .unwrap();

        // Check the handle path
        assert_eq!(handle.to_string(), "outstanding/0");

        // Request should be queued
        assert_eq!(broker.handle_count(), 1);
    }

    #[test]
    fn test_broker_with_mock_executor() {
        let mock = MockExecutor::new().with_response(
            "https://api.example.com/users",
            MockExecutor::success_response(serde_json::json!({"users": ["alice", "bob"]})),
        );

        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request
        let request = HttpRequest::get("https://api.example.com/users");
        let request_value = to_value(&request).unwrap();
        let handle = broker
            .write(&path!(""), Record::parsed(request_value))
            .unwrap();

        // Execute and get response
        let record = broker.read(&handle).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        let response: HttpResponse = from_value(value).unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(
            response.body,
            serde_json::json!({"users": ["alice", "bob"]})
        );
    }

    #[test]
    fn test_broker_multiple_requests() {
        let mock = MockExecutor::new()
            .with_response(
                "/a",
                MockExecutor::success_response(serde_json::json!({"id": "a"})),
            )
            .with_response(
                "/b",
                MockExecutor::success_response(serde_json::json!({"id": "b"})),
            );

        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue two requests
        let h1 = broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/a")).unwrap()),
            )
            .unwrap();
        let h2 = broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/b")).unwrap()),
            )
            .unwrap();

        assert_eq!(h1.to_string(), "outstanding/0");
        assert_eq!(h2.to_string(), "outstanding/1");
        assert_eq!(broker.handle_count(), 2);

        // Execute in reverse order
        let r2 = broker.read(&h2).unwrap().unwrap();
        let v2: HttpResponse = from_value(r2.into_value(&NoCodec).unwrap()).unwrap();
        assert_eq!(v2.body, serde_json::json!({"id": "b"}));

        let r1 = broker.read(&h1).unwrap().unwrap();
        let v1: HttpResponse = from_value(r1.into_value(&NoCodec).unwrap()).unwrap();
        assert_eq!(v1.body, serde_json::json!({"id": "a"}));
    }

    #[test]
    fn test_broker_request_not_found() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        let result = broker.read(&path!("outstanding/999"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_broker_invalid_path() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        let result = broker.read(&path!("invalid"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid path"));
    }

    #[test]
    fn test_broker_http_error() {
        let mock = MockExecutor::new().fail_with("Connection refused");
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request
        let request = HttpRequest::get("https://example.com");
        let handle = broker
            .write(&path!(""), Record::parsed(to_value(&request).unwrap()))
            .unwrap();

        // Execute should fail
        let result = broker.read(&handle);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Connection refused"));
    }

    #[test]
    fn test_broker_invalid_request_data() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Write invalid data (not an HttpRequest)
        let result = broker.write(
            &path!(""),
            Record::parsed(to_value(&"not a request").unwrap()),
        );
        assert!(result.is_err());
    }

    // ==================== Idempotency tests ====================

    #[test]
    fn test_broker_idempotent_read() {
        let mock = MockExecutor::new().with_response(
            "https://api.example.com/data",
            MockExecutor::success_response(serde_json::json!({"value": 42})),
        );

        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request
        let request = HttpRequest::get("https://api.example.com/data");
        let handle = broker
            .write(&path!(""), Record::parsed(to_value(&request).unwrap()))
            .unwrap();

        // First read - executes and caches
        let r1 = broker.read(&handle).unwrap().unwrap();
        let v1 = r1.into_value(&NoCodec).unwrap();

        // Second read - returns cached (idempotent)
        let r2 = broker.read(&handle).unwrap().unwrap();
        let v2 = r2.into_value(&NoCodec).unwrap();

        // Results should be identical
        assert_eq!(v1, v2);

        // Handle should still exist
        assert!(broker.has_handle(0));
    }

    #[test]
    fn test_broker_idempotent_read_error_cached() {
        let mock = MockExecutor::new().fail_with("Network timeout");
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request
        let request = HttpRequest::get("https://example.com");
        let handle = broker
            .write(&path!(""), Record::parsed(to_value(&request).unwrap()))
            .unwrap();

        // First read - fails and caches error
        let r1 = broker.read(&handle);
        assert!(r1.is_err());
        assert!(r1.unwrap_err().to_string().contains("Network timeout"));

        // Second read - returns same cached error
        let r2 = broker.read(&handle);
        assert!(r2.is_err());
        assert!(r2.unwrap_err().to_string().contains("Network timeout"));
    }

    #[test]
    fn test_broker_list_outstanding() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue multiple requests
        broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/a")).unwrap()),
            )
            .unwrap();
        broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/b")).unwrap()),
            )
            .unwrap();
        broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/c")).unwrap()),
            )
            .unwrap();

        // List outstanding
        let result = broker.read(&path!("outstanding")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            structfs_core_store::Value::Array(ids) => {
                assert_eq!(ids.len(), 3);
                assert_eq!(ids[0], structfs_core_store::Value::Integer(0));
                assert_eq!(ids[1], structfs_core_store::Value::Integer(1));
                assert_eq!(ids[2], structfs_core_store::Value::Integer(2));
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_broker_view_queued_request() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request with specific details
        let request = HttpRequest::get("https://api.example.com/users")
            .with_header("Authorization", "Bearer token123");
        let handle = broker
            .write(&path!(""), Record::parsed(to_value(&request).unwrap()))
            .unwrap();

        // Read the queued request (not the response)
        let request_path = handle.join(&path!("request"));
        let result = broker.read(&request_path).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();
        let retrieved: HttpRequest = from_value(value).unwrap();

        // Should match what we queued
        assert_eq!(retrieved.method, crate::types::Method::GET);
        assert_eq!(retrieved.path, "https://api.example.com/users");
        assert_eq!(
            retrieved.headers.get("Authorization"),
            Some(&"Bearer token123".to_string())
        );
    }

    #[test]
    fn test_broker_unknown_subpath() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request
        broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/test")).unwrap()),
            )
            .unwrap();

        // Try to read unknown sub-path
        let result = broker.read(&path!("outstanding/0/unknown"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown sub-path"));
    }

    #[test]
    fn test_broker_delete_handle() {
        let mock = MockExecutor::new().with_response(
            "/test",
            MockExecutor::success_response(serde_json::json!({"ok": true})),
        );
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue and execute a request
        let handle = broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/test")).unwrap()),
            )
            .unwrap();
        broker.read(&handle).unwrap(); // Execute to cache response

        // Handle should exist
        assert!(broker.has_handle(0));

        // Delete by writing null
        broker
            .write(&handle, Record::parsed(structfs_core_store::Value::Null))
            .unwrap();

        // Handle should be gone
        assert!(!broker.has_handle(0));

        // Reading should now fail
        let result = broker.read(&handle);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_broker_delete_updates_listing() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue two requests
        let h1 = broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/a")).unwrap()),
            )
            .unwrap();
        broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/b")).unwrap()),
            )
            .unwrap();

        // Delete first
        broker
            .write(&h1, Record::parsed(structfs_core_store::Value::Null))
            .unwrap();

        // Listing should only show second
        let result = broker.read(&path!("outstanding")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            structfs_core_store::Value::Array(ids) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0], structfs_core_store::Value::Integer(1));
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_broker_cannot_overwrite_request() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Queue a request
        let handle = broker
            .write(
                &path!(""),
                Record::parsed(to_value(&HttpRequest::get("/original")).unwrap()),
            )
            .unwrap();

        // Try to overwrite with non-null value
        let result = broker.write(
            &handle,
            Record::parsed(to_value(&HttpRequest::get("/replacement")).unwrap()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot overwrite"));
    }

    #[test]
    fn test_broker_invalid_write_path() {
        let mock = MockExecutor::new();
        let mut broker = HttpBrokerStore::with_executor(mock);

        // Try to write to invalid path
        let result = broker.write(
            &path!("something/else"),
            Record::parsed(to_value(&HttpRequest::get("/test")).unwrap()),
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid write path"));
    }

    // ==================== HttpClientStore tests ====================

    #[test]
    fn test_client_store_with_mock() {
        let mock = MockExecutor::new().with_response(
            "https://api.example.com/users/1",
            MockExecutor::success_response(serde_json::json!({"id": 1, "name": "Alice"})),
        );

        let mut client = HttpClientStore::with_executor("https://api.example.com", mock).unwrap();

        let record = client.read(&path!("users/1")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        let expected = to_value(&serde_json::json!({"id": 1, "name": "Alice"})).unwrap();

        assert_eq!(value, expected);
    }

    #[test]
    fn test_client_store_404_returns_none() {
        let mock = MockExecutor::new()
            .with_response("https://api.example.com/missing", MockExecutor::not_found());

        let mut client = HttpClientStore::with_executor("https://api.example.com", mock).unwrap();

        let result = client.read(&path!("missing")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_client_store_server_error() {
        let mock = MockExecutor::new().with_response(
            "https://api.example.com/error",
            MockExecutor::error_response(500, "Internal Server Error"),
        );

        let mut client = HttpClientStore::with_executor("https://api.example.com", mock).unwrap();

        let result = client.read(&path!("error"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("500"));
    }

    #[test]
    fn test_client_store_write_post() {
        let mock = MockExecutor::new().with_default_response(MockExecutor::success_response(
            serde_json::json!({"created": true}),
        ));

        let mut client =
            HttpClientStore::with_executor("https://api.example.com", mock.clone()).unwrap();

        let data = serde_json::json!({"name": "Bob"});
        let result = client.write(&path!("users"), Record::parsed(to_value(&data).unwrap()));
        assert!(result.is_ok());

        let requests = mock.recorded_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, crate::types::Method::POST);
    }

    #[test]
    fn test_client_store_write_http_request() {
        let mock = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let mut client =
            HttpClientStore::with_executor("https://api.example.com", mock.clone()).unwrap();

        // Write an HttpRequest directly to root
        let request = HttpRequest::delete("/users/1");
        let result = client.write(&path!(""), Record::parsed(to_value(&request).unwrap()));
        assert!(result.is_ok());

        let requests = mock.recorded_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, crate::types::Method::DELETE);
    }

    #[test]
    fn test_client_store_default_headers() {
        let mock = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let mut client = HttpClientStore::with_executor("https://api.example.com", mock.clone())
            .unwrap()
            .with_default_header("Authorization", "Bearer token123");

        client.read(&path!("data")).unwrap();

        let requests = mock.recorded_requests();
        assert_eq!(
            requests[0].headers.get("Authorization"),
            Some(&"Bearer token123".to_string())
        );
    }

    #[test]
    fn test_client_store_base_url_resolution() {
        let mock = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let client =
            HttpClientStore::with_executor("https://api.example.com/v1/", mock.clone()).unwrap();

        let request = HttpRequest::get("users");
        let full_request = client.build_request(request);

        assert_eq!(full_request.path, "https://api.example.com/v1/users");
    }

    #[test]
    fn test_client_store_absolute_url_preserved() {
        let mock = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let client =
            HttpClientStore::with_executor("https://api.example.com", mock.clone()).unwrap();

        let request = HttpRequest::get("https://other.com/data");
        let full_request = client.build_request(request);

        assert_eq!(full_request.path, "https://other.com/data");
    }

    #[test]
    fn test_client_store_request_headers_override_default() {
        let mock = MockExecutor::new()
            .with_default_response(MockExecutor::success_response(serde_json::Value::Null));

        let client = HttpClientStore::with_executor("https://api.example.com", mock.clone())
            .unwrap()
            .with_default_header("X-Custom", "default");

        let request = HttpRequest::get("/data").with_header("X-Custom", "override");
        let full_request = client.build_request(request);

        assert_eq!(
            full_request.headers.get("X-Custom"),
            Some(&"override".to_string())
        );
    }

    #[test]
    fn test_client_store_write_failure() {
        let mock = MockExecutor::new()
            .with_default_response(MockExecutor::error_response(400, "Bad Request"));

        let mut client = HttpClientStore::with_executor("https://api.example.com", mock).unwrap();

        let result = client.write(
            &path!("data"),
            Record::parsed(to_value(&serde_json::json!({})).unwrap()),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("400"));
    }

    // ==================== AsyncHttpBrokerStore tests ====================

    #[test]
    fn test_async_broker_parse_handle_path() {
        assert_eq!(
            AsyncHttpBrokerStore::parse_handle_path(&path!("outstanding/0")),
            Some((0, None))
        );
        assert_eq!(
            AsyncHttpBrokerStore::parse_handle_path(&path!("outstanding/123/response")),
            Some((123, Some("response".to_string())))
        );
        assert_eq!(
            AsyncHttpBrokerStore::parse_handle_path(&path!("outstanding")),
            None
        );
        assert_eq!(
            AsyncHttpBrokerStore::parse_handle_path(&path!("other/123")),
            None
        );
        assert_eq!(AsyncHttpBrokerStore::parse_handle_path(&path!("")), None);
        assert_eq!(
            AsyncHttpBrokerStore::parse_handle_path(&path!("outstanding/abc")),
            None
        );
        assert_eq!(
            AsyncHttpBrokerStore::parse_handle_path(&path!("outstanding/0/foo/bar")),
            Some((0, Some("foo/bar".to_string())))
        );
    }

    #[test]
    fn test_async_broker_queue_request() {
        let mut broker = AsyncHttpBrokerStore::with_default_timeout().unwrap();

        // Create a request
        let request_value = to_value(&HttpRequest::get("https://example.com")).unwrap();

        // Write the request (starts executing in background)
        let handle = broker
            .write(&path!(""), Record::parsed(request_value))
            .unwrap();

        // Check the handle path
        assert_eq!(handle.to_string(), "outstanding/0");

        // Should be able to read the status (pending initially)
        let status = broker.read(&handle).unwrap();
        assert!(status.is_some());
    }

    #[test]
    fn test_async_broker_invalid_path() {
        let mut broker = AsyncHttpBrokerStore::with_default_timeout().unwrap();

        let result = broker.read(&path!("invalid"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid handle path"));
    }

    #[test]
    fn test_async_broker_request_not_found() {
        let mut broker = AsyncHttpBrokerStore::with_default_timeout().unwrap();

        let result = broker.read(&path!("outstanding/999"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_async_broker_unknown_subpath() {
        let mut broker = AsyncHttpBrokerStore::with_default_timeout().unwrap();

        // Queue a request
        let request_value = to_value(&HttpRequest::get("https://example.com")).unwrap();
        broker
            .write(&path!(""), Record::parsed(request_value))
            .unwrap();

        let result = broker.read(&path!("outstanding/0/unknown"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown sub-path"));
    }

    #[test]
    fn test_async_broker_response_not_ready() {
        let mut broker = AsyncHttpBrokerStore::with_default_timeout().unwrap();

        // Queue a request that will timeout/fail (we don't wait for it)
        let request_value = to_value(&HttpRequest::get("https://example.com")).unwrap();
        broker
            .write(&path!(""), Record::parsed(request_value))
            .unwrap();

        // Immediately try to get response (before background thread completes)
        let result = broker.read(&path!("outstanding/0/response")).unwrap();
        // It might be None if not ready yet
        // We just check it doesn't error
        let _ = result;
    }

    #[test]
    fn test_async_broker_invalid_request_data() {
        let mut broker = AsyncHttpBrokerStore::with_default_timeout().unwrap();

        let result = broker.write(
            &path!(""),
            Record::parsed(to_value(&"not a request").unwrap()),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_async_broker_custom_timeout() {
        let broker = AsyncHttpBrokerStore::new(Duration::from_secs(5)).unwrap();
        assert_eq!(broker.timeout, Duration::from_secs(5));
    }

    // ==================== Production executor tests ====================

    #[test]
    fn test_reqwest_executor_creation() {
        let executor = ReqwestExecutor::with_default_timeout();
        assert!(executor.is_ok());
    }

    #[test]
    fn test_reqwest_executor_custom_timeout() {
        let executor = ReqwestExecutor::new(Duration::from_secs(10));
        assert!(executor.is_ok());
    }

    #[test]
    fn test_broker_store_creation() {
        let broker = HttpBrokerStore::with_default_timeout();
        assert!(broker.is_ok());
    }

    #[test]
    fn test_client_store_creation() {
        let client = HttpClientStore::new("https://example.com");
        assert!(client.is_ok());
    }

    #[test]
    fn test_client_store_invalid_url() {
        let client = HttpClientStore::new("not a url");
        assert!(client.is_err());
    }
}
