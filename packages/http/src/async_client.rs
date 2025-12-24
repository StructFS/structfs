//! Async HTTP client store
//!
//! This store provides a synchronous StructFS interface to async HTTP operations.
//! Requests are executed asynchronously, with status queryable through handle paths.
//!
//! # Usage Model
//!
//! ```text
//! 1. Write an HttpRequest to initiate:
//!    write("", request) -> returns "handles/{id}"
//!
//! 2. Query status (non-blocking):
//!    read("handles/{id}") -> RequestStatus { state: pending|complete|failed, ... }
//!
//! 3. Read response (returns None if still pending):
//!    read("handles/{id}/response") -> Option<HttpResponse>
//!
//! 4. Block until complete:
//!    write("handles/{id}/await", ()) -> blocks, returns "handles/{id}/response"
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::runtime::Runtime;
use url::Url;

use structfs_store::{Error as StoreError, Path, Reader, Writer};

use crate::error::Error;
use crate::handle::{HandleState, RequestStatus};
use crate::types::{HttpRequest, HttpResponse};

/// Shared state for a single request handle
struct SharedHandle {
    state: Mutex<HandleState>,
    completed: Condvar,
}

impl SharedHandle {
    fn new(id: String) -> Self {
        Self {
            state: Mutex::new(HandleState::new(id)),
            completed: Condvar::new(),
        }
    }

    fn get_status(&self) -> RequestStatus {
        self.state.lock().unwrap().status.clone()
    }

    fn get_response(&self) -> Option<HttpResponse> {
        self.state.lock().unwrap().response.clone()
    }

    fn complete(&self, response: HttpResponse) {
        let mut state = self.state.lock().unwrap();
        state.complete(response);
        self.completed.notify_all();
    }

    fn fail(&self, error: String) {
        let mut state = self.state.lock().unwrap();
        state.fail(error);
        self.completed.notify_all();
    }

    fn wait_for_completion(&self) -> RequestStatus {
        let mut state = self.state.lock().unwrap();
        while state.status.is_pending() {
            state = self.completed.wait(state).unwrap();
        }
        state.status.clone()
    }

    fn wait_for_completion_timeout(&self, timeout: Duration) -> Option<RequestStatus> {
        let mut state = self.state.lock().unwrap();
        while state.status.is_pending() {
            let result = self.completed.wait_timeout(state, timeout).unwrap();
            state = result.0;
            if result.1.timed_out() {
                return None;
            }
        }
        Some(state.status.clone())
    }
}

/// An async HTTP client store using the StructFS handle pattern
///
/// This store executes HTTP requests asynchronously while providing a synchronous
/// StructFS interface. Requests return handle paths that can be queried for status
/// or blocked on for completion.
///
/// # Example
///
/// ```ignore
/// use structfs_http::async_client::AsyncHttpClientStore;
/// use structfs_http::HttpRequest;
/// use structfs_store::{Reader, Writer, Path};
///
/// let mut store = AsyncHttpClientStore::new("https://api.example.com")?;
///
/// // Initiate an async request
/// let request = HttpRequest::get("users/123");
/// let handle_path = store.write(&Path::parse("")?, &request)?;
/// // handle_path = "handles/{uuid}"
///
/// // Check status (non-blocking)
/// let status: RequestStatus = store.read_owned(&handle_path)?;
/// println!("State: {:?}", status.state);
///
/// // Block until complete
/// store.write(&handle_path.join(&Path::parse("await")?), &())?;
///
/// // Read the response
/// let response: HttpResponse = store.read_owned(
///     &handle_path.join(&Path::parse("response")?)
/// )?.unwrap();
/// ```
pub struct AsyncHttpClientStore {
    client: Client,
    base_url: Url,
    runtime: Runtime,
    handles: Arc<Mutex<HashMap<String, Arc<SharedHandle>>>>,
    default_headers: HashMap<String, String>,
    next_id: Mutex<u64>,
}

impl AsyncHttpClientStore {
    /// Create a new async HTTP client store
    pub fn new(base_url: &str) -> Result<Self, Error> {
        let base_url = Url::parse(base_url)?;
        let client = Client::new();
        let runtime = Runtime::new().map_err(|e| Error::InvalidUrl {
            message: format!("Failed to create tokio runtime: {}", e),
        })?;

        Ok(Self {
            client,
            base_url,
            runtime,
            handles: Arc::new(Mutex::new(HashMap::new())),
            default_headers: HashMap::new(),
            next_id: Mutex::new(0),
        })
    }

    /// Add a default header sent with every request
    pub fn with_default_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.insert(name.into(), value.into());
        self
    }

    /// Generate a unique request ID
    fn generate_id(&self) -> String {
        let mut id = self.next_id.lock().unwrap();
        let current = *id;
        *id += 1;
        format!("{:016x}", current)
    }

    /// Execute an HTTP request asynchronously
    fn execute_request_async(&self, request: HttpRequest) -> String {
        let id = self.generate_id();
        let handle = Arc::new(SharedHandle::new(id.clone()));

        // Store the handle
        {
            let mut handles = self.handles.lock().unwrap();
            handles.insert(id.clone(), handle.clone());
        }

        // Build the request
        let url = if request.path.starts_with("http://") || request.path.starts_with("https://") {
            request.path.clone()
        } else {
            self.base_url
                .join(&request.path)
                .map(|u| u.to_string())
                .unwrap_or_else(|_| request.path.clone())
        };

        let client = self.client.clone();
        let method: http::Method = request.method.clone().into();
        let query = request.query.clone();
        let mut headers = self.default_headers.clone();
        headers.extend(request.headers.clone());
        let body = request.body.clone();

        // Spawn the async task
        self.runtime.spawn(async move {
            let result = async {
                let mut req_builder = client.request(method, &url);

                if !query.is_empty() {
                    req_builder = req_builder.query(&query);
                }

                for (name, value) in &headers {
                    req_builder = req_builder.header(name, value);
                }

                if let Some(body) = body {
                    req_builder = req_builder.json(&body);
                }

                let response = req_builder.send().await?;

                let status = response.status().as_u16();
                let status_text = response
                    .status()
                    .canonical_reason()
                    .unwrap_or("Unknown")
                    .to_string();

                let mut resp_headers = HashMap::new();
                for (name, value) in response.headers() {
                    if let Ok(v) = value.to_str() {
                        resp_headers.insert(name.to_string(), v.to_string());
                    }
                }

                let body_text = response.text().await?;
                let body = serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);

                Ok::<_, reqwest::Error>(HttpResponse {
                    status,
                    status_text,
                    headers: resp_headers,
                    body,
                    body_text: Some(body_text),
                })
            }
            .await;

            match result {
                Ok(response) => handle.complete(response),
                Err(e) => handle.fail(e.to_string()),
            }
        });

        id
    }

    /// Get a handle by ID
    fn get_handle(&self, id: &str) -> Option<Arc<SharedHandle>> {
        self.handles.lock().unwrap().get(id).cloned()
    }

    /// Parse a handle path into (id, subpath)
    fn parse_handle_path(&self, path: &Path) -> Option<(String, Option<String>)> {
        if path.components.is_empty() {
            return None;
        }

        if path.components[0] != "handles" {
            return None;
        }

        if path.components.len() < 2 {
            return None;
        }

        let id = path.components[1].clone();
        let subpath = if path.components.len() > 2 {
            Some(path.components[2..].join("/"))
        } else {
            None
        };

        Some((id, subpath))
    }
}

impl Reader for AsyncHttpClientStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        // Parse the path
        let (id, subpath) = self.parse_handle_path(from).ok_or_else(|| {
            StoreError::PathError(structfs_store::PathError::PathInvalid {
                path: from.clone(),
                message: "Invalid handle path. Expected: handles/{id}[/response]".to_string(),
            })
        })?;

        let handle = self.get_handle(&id).ok_or_else(|| {
            StoreError::PathError(structfs_store::PathError::PathInvalid {
                path: from.clone(),
                message: format!("Handle '{}' not found", id),
            })
        })?;

        match subpath.as_deref() {
            None => {
                // Read status
                let status = handle.get_status();
                let value = serde_json::to_value(&status).map_err(|e| {
                    StoreError::RecordSerialization {
                        message: e.to_string(),
                    }
                })?;
                let de: Box<dyn erased_serde::Deserializer> =
                    Box::new(<dyn erased_serde::Deserializer>::erase(value));
                Ok(Some(de))
            }
            Some("response") => {
                // Read response (may be None if pending)
                match handle.get_response() {
                    Some(response) => {
                        let value = serde_json::to_value(&response).map_err(|e| {
                            StoreError::RecordSerialization {
                                message: e.to_string(),
                            }
                        })?;
                        let de: Box<dyn erased_serde::Deserializer> =
                            Box::new(<dyn erased_serde::Deserializer>::erase(value));
                        Ok(Some(de))
                    }
                    None => Ok(None),
                }
            }
            Some(other) => Err(StoreError::PathError(
                structfs_store::PathError::PathInvalid {
                    path: from.clone(),
                    message: format!("Unknown handle subpath: {}", other),
                },
            )),
        }
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        if let Some(mut de) = self.read_to_deserializer(from)? {
            let record = erased_serde::deserialize(&mut *de).map_err(|e| {
                StoreError::RecordDeserialization {
                    message: e.to_string(),
                }
            })?;
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }
}

impl Writer for AsyncHttpClientStore {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        // Check if this is a write to initiate a new request (root path)
        if destination.is_empty() {
            // Try to interpret as HttpRequest
            let value =
                serde_json::to_value(&data).map_err(|e| StoreError::RecordSerialization {
                    message: e.to_string(),
                })?;

            let request: HttpRequest =
                serde_json::from_value(value).map_err(|e| StoreError::RecordDeserialization {
                    message: format!("Expected HttpRequest: {}", e),
                })?;

            let id = self.execute_request_async(request);
            return Ok(Path::parse(&format!("handles/{}", id)).unwrap());
        }

        // Check if this is a write to a handle path (e.g., handles/{id}/await)
        if let Some((id, subpath)) = self.parse_handle_path(destination) {
            let handle = self.get_handle(&id).ok_or_else(|| {
                StoreError::PathError(structfs_store::PathError::PathInvalid {
                    path: destination.clone(),
                    message: format!("Handle '{}' not found", id),
                })
            })?;

            match subpath.as_deref() {
                Some("await") => {
                    // Block until the request completes
                    let _status = handle.wait_for_completion();
                    return Ok(Path::parse(&format!("handles/{}/response", id)).unwrap());
                }
                Some("await_timeout") => {
                    // Try to parse timeout from data
                    let value = serde_json::to_value(&data).map_err(|e| {
                        StoreError::RecordSerialization {
                            message: e.to_string(),
                        }
                    })?;

                    let timeout_ms: u64 = value.as_u64().unwrap_or(30000);
                    let timeout = Duration::from_millis(timeout_ms);

                    match handle.wait_for_completion_timeout(timeout) {
                        Some(_status) => {
                            Ok(Path::parse(&format!("handles/{}/response", id)).unwrap())
                        }
                        None => Err(StoreError::ImplementationFailure {
                            message: format!("Request {} timed out after {}ms", id, timeout_ms),
                        }),
                    }
                }
                _ => Err(StoreError::PathError(
                    structfs_store::PathError::PathNotWritable {
                        path: destination.clone(),
                        message: "Can only write to 'await' or 'await_timeout' subpaths"
                            .to_string(),
                    },
                )),
            }
        } else {
            Err(StoreError::PathError(
                structfs_store::PathError::PathNotWritable {
                    path: destination.clone(),
                    message: "Write to root path with HttpRequest to initiate a request"
                        .to_string(),
                },
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_id() {
        let store = AsyncHttpClientStore::new("https://example.com").unwrap();
        let id1 = store.generate_id();
        let id2 = store.generate_id();
        assert_ne!(id1, id2);
        assert_eq!(id1.len(), 16);
    }

    #[test]
    fn test_parse_handle_path() {
        let store = AsyncHttpClientStore::new("https://example.com").unwrap();

        // Valid paths
        let path = Path::parse("handles/abc123").unwrap();
        let (id, subpath) = store.parse_handle_path(&path).unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(subpath, None);

        let path = Path::parse("handles/abc123/response").unwrap();
        let (id, subpath) = store.parse_handle_path(&path).unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(subpath, Some("response".to_string()));

        let path = Path::parse("handles/abc123/await").unwrap();
        let (id, subpath) = store.parse_handle_path(&path).unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(subpath, Some("await".to_string()));

        // Invalid paths
        let path = Path::parse("").unwrap();
        assert!(store.parse_handle_path(&path).is_none());

        let path = Path::parse("other/path").unwrap();
        assert!(store.parse_handle_path(&path).is_none());

        let path = Path::parse("handles").unwrap();
        assert!(store.parse_handle_path(&path).is_none());
    }
}
