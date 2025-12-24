//! Async HTTP broker store that executes requests in background threads.
//!
//! Unlike the sync broker, this store executes requests immediately in a background
//! thread when you write them. Reading from the handle returns the status, and
//! reading from `handle/response` returns the actual response when complete.
//!
//! ## Example
//!
//! ```ignore
//! use structfs_http::async_broker::AsyncHttpBrokerStore;
//! use structfs_http::HttpRequest;
//! use structfs_store::{Reader, Writer, Path};
//!
//! let mut broker = AsyncHttpBrokerStore::new(Duration::from_secs(30))?;
//!
//! // Queue requests (they start executing immediately in background)
//! let h1 = broker.write(&path!(""), &HttpRequest::get("https://api.example.com/a"))?;
//! let h2 = broker.write(&path!(""), &HttpRequest::get("https://api.example.com/b"))?;
//!
//! // Check status
//! let status: RequestStatus = broker.read_owned(&h1)?.unwrap();
//! if status.is_complete() {
//!     let response: HttpResponse = broker.read_owned(&h1.join(&path!("response")))?.unwrap();
//! }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_store::{path, Error as StoreError, Path, Reader, Writer};

use crate::handle::RequestStatus;
use crate::types::{HttpRequest, HttpResponse};
use crate::Error;

const OUTSTANDING_PREFIX: &str = "outstanding";

type RequestId = u64;

/// Internal state for a request
struct RequestHandle {
    status: RequestStatus,
    response: Option<HttpResponse>,
}

/// Async HTTP broker store.
///
/// Requests are executed in background threads. Write to queue a request,
/// read from the handle to check status or get the response.
pub struct AsyncHttpBrokerStore {
    handles: Arc<Mutex<HashMap<RequestId, RequestHandle>>>,
    next_request_id: RequestId,
    timeout: Duration,
}

impl AsyncHttpBrokerStore {
    /// Create a new async HTTP broker store with the given request timeout.
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        Ok(Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            next_request_id: 0,
            timeout,
        })
    }

    /// Create with default timeout of 30 seconds.
    pub fn with_default_timeout() -> Result<Self, Error> {
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
        if path.components.is_empty() || path.components[0] != OUTSTANDING_PREFIX {
            return None;
        }
        if path.components.len() < 2 {
            return None;
        }
        let id: RequestId = path.components[1].parse().ok()?;
        let sub_path = if path.components.len() > 2 {
            Some(path.components[2..].join("/"))
        } else {
            None
        };
        Some((id, sub_path))
    }
}

impl Reader for AsyncHttpBrokerStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        let (request_id, sub_path) =
            Self::parse_handle_path(from).ok_or_else(|| StoreError::Raw {
                message: format!(
                    "Invalid handle path '{}'. Expected format: outstanding/{{id}}[/response]",
                    from
                ),
            })?;

        let handles = self.handles.lock().map_err(|e| StoreError::Raw {
            message: format!("Lock error: {}", e),
        })?;

        let handle = handles.get(&request_id).ok_or_else(|| StoreError::Raw {
            message: format!("Request with ID {} not found", request_id),
        })?;

        let json = match sub_path.as_deref() {
            Some("response") => {
                if let Some(ref response) = handle.response {
                    serde_json::to_value(response)
                } else {
                    return Ok(None); // Response not ready yet
                }
            }
            None => serde_json::to_value(&handle.status),
            Some(other) => {
                return Err(StoreError::Raw {
                    message: format!(
                        "Unknown sub-path '{}'. Use 'response' to get the response.",
                        other
                    ),
                });
            }
        }
        .map_err(|e| StoreError::RecordSerialization {
            message: e.to_string(),
        })?;

        Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
            json,
        ))))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        let (request_id, sub_path) =
            Self::parse_handle_path(from).ok_or_else(|| StoreError::Raw {
                message: format!(
                    "Invalid handle path '{}'. Expected format: outstanding/{{id}}[/response]",
                    from
                ),
            })?;

        let handles = self.handles.lock().map_err(|e| StoreError::Raw {
            message: format!("Lock error: {}", e),
        })?;

        let handle = handles.get(&request_id).ok_or_else(|| StoreError::Raw {
            message: format!("Request with ID {} not found", request_id),
        })?;

        let json = match sub_path.as_deref() {
            Some("response") => {
                if let Some(ref response) = handle.response {
                    serde_json::to_value(response)
                } else {
                    return Ok(None); // Response not ready yet
                }
            }
            None => serde_json::to_value(&handle.status),
            Some(other) => {
                return Err(StoreError::Raw {
                    message: format!(
                        "Unknown sub-path '{}'. Use 'response' to get the response.",
                        other
                    ),
                });
            }
        }
        .map_err(|e| StoreError::RecordSerialization {
            message: e.to_string(),
        })?;

        let record =
            serde_json::from_value(json).map_err(|e| StoreError::RecordDeserialization {
                message: e.to_string(),
            })?;

        Ok(Some(record))
    }
}

impl Writer for AsyncHttpBrokerStore {
    fn write<RecordType: Serialize>(
        &mut self,
        _destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        // Convert data to JSON Value then to HttpRequest
        let json = serde_json::to_value(&data).map_err(|e| StoreError::RecordSerialization {
            message: e.to_string(),
        })?;

        let request: HttpRequest =
            serde_json::from_value(json).map_err(|e| StoreError::RecordDeserialization {
                message: format!("Data must be an HttpRequest: {}", e),
            })?;

        // Create initial pending status
        let handle = RequestHandle {
            status: RequestStatus::pending(request_id.to_string()),
            response: None,
        };

        {
            let mut handles = self.handles.lock().map_err(|e| StoreError::Raw {
                message: format!("Lock error: {}", e),
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

    #[test]
    fn test_parse_handle_path() {
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
    }
}
