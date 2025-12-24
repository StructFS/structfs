//! HTTP broker store that queues requests and executes them on read.
//!
//! This store provides a deferred execution pattern:
//! - **Write** an `HttpRequest` → returns a handle path like `outstanding/{id}`
//! - **Read** from `outstanding/{id}` → executes the request and returns `HttpResponse`
//!
//! This allows encoding any HTTP request (method, headers, body, URL) through
//! the StructFS read/write interface.
//!
//! ## Example
//!
//! ```ignore
//! use structfs_http::broker::HttpBrokerStore;
//! use structfs_http::HttpRequest;
//! use structfs_store::{Reader, Writer, Path};
//!
//! let mut broker = HttpBrokerStore::new(Duration::from_secs(30))?;
//!
//! // Queue a request
//! let request = HttpRequest::get("https://api.example.com/users/123")
//!     .with_header("Authorization", "Bearer token");
//! let handle = broker.write(&Path::parse("")?, &request)?;
//! // handle = "outstanding/0"
//!
//! // Execute the request by reading from the handle
//! let response: HttpResponse = broker.read_owned(&handle)?.unwrap();
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_store::{path, Error as StoreError, Path, Reader, Writer};

use crate::types::{HttpRequest, HttpResponse};
use crate::Error;

const OUTSTANDING_PREFIX: &str = "outstanding";

type RequestId = u64;

/// HTTP broker store for sync (blocking) requests.
///
/// Write requests are queued and executed when reading from the handle path.
pub struct HttpBrokerStore {
    requests: BTreeMap<RequestId, HttpRequest>,
    next_request_id: RequestId,
    http_client: Client,
}

impl HttpBrokerStore {
    /// Create a new HTTP broker store with the given request timeout.
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        let http_client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(Error::from)?;

        Ok(Self {
            requests: BTreeMap::new(),
            next_request_id: 0,
            http_client,
        })
    }

    /// Create with default timeout of 30 seconds.
    pub fn with_default_timeout() -> Result<Self, Error> {
        Self::new(Duration::from_secs(30))
    }

    /// Execute an HTTP request and return the response.
    fn execute_request(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let method: http::Method = request.method.into();

        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let header_name =
                HeaderName::try_from(name.as_str()).map_err(|e| Error::InvalidUrl {
                    message: e.to_string(),
                })?;
            let header_value =
                HeaderValue::try_from(value.as_str()).map_err(|e| Error::InvalidUrl {
                    message: e.to_string(),
                })?;
            headers.insert(header_name, header_value);
        }

        let mut req_builder = self.http_client.request(method, &request.path);
        req_builder = req_builder.headers(headers);

        if !request.query.is_empty() {
            req_builder = req_builder.query(&request.query);
        }

        if let Some(body) = &request.body {
            req_builder = req_builder.json(body);
        }

        let response = req_builder.send()?;

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

        let body_text = response.text()?;
        let body = serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);

        Ok(HttpResponse {
            status,
            status_text,
            headers: resp_headers,
            body,
            body_text: Some(body_text),
        })
    }

    /// Parse request ID from a path like "outstanding/123".
    fn parse_request_id(path: &Path) -> Option<RequestId> {
        if path.components.len() != 2 {
            return None;
        }
        if path.components[0] != OUTSTANDING_PREFIX {
            return None;
        }
        path.components[1].parse().ok()
    }
}

impl Reader for HttpBrokerStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        let request_id = Self::parse_request_id(from).ok_or_else(|| StoreError::Raw {
            message: format!(
                "Invalid handle path '{}'. Expected format: outstanding/{{id}}",
                from
            ),
        })?;

        let request = self
            .requests
            .remove(&request_id)
            .ok_or_else(|| StoreError::Raw {
                message: format!("Request with ID {} not found", request_id),
            })?;

        let response = self.execute_request(request).map_err(|e| StoreError::Raw {
            message: format!("HTTP request failed: {}", e),
        })?;

        let json =
            serde_json::to_value(&response).map_err(|e| StoreError::RecordSerialization {
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
        let request_id = Self::parse_request_id(from).ok_or_else(|| StoreError::Raw {
            message: format!(
                "Invalid handle path '{}'. Expected format: outstanding/{{id}}",
                from
            ),
        })?;

        let request = self
            .requests
            .remove(&request_id)
            .ok_or_else(|| StoreError::Raw {
                message: format!("Request with ID {} not found", request_id),
            })?;

        let response = self.execute_request(request).map_err(|e| StoreError::Raw {
            message: format!("HTTP request failed: {}", e),
        })?;

        let json =
            serde_json::to_value(&response).map_err(|e| StoreError::RecordSerialization {
                message: e.to_string(),
            })?;

        let record =
            serde_json::from_value(json).map_err(|e| StoreError::RecordDeserialization {
                message: e.to_string(),
            })?;

        Ok(Some(record))
    }
}

impl Writer for HttpBrokerStore {
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

        self.requests.insert(request_id, request);

        Ok(path!(OUTSTANDING_PREFIX).join(&path!(&format!("{}", request_id))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_request_id() {
        assert_eq!(
            HttpBrokerStore::parse_request_id(&path!("outstanding/0")),
            Some(0)
        );
        assert_eq!(
            HttpBrokerStore::parse_request_id(&path!("outstanding/123")),
            Some(123)
        );
        assert_eq!(
            HttpBrokerStore::parse_request_id(&path!("outstanding")),
            None
        );
        assert_eq!(HttpBrokerStore::parse_request_id(&path!("other/123")), None);
    }
}
