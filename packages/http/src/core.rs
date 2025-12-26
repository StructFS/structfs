//! New architecture implementations of HTTP stores using core-store.
//!
//! This module provides implementations using the new three-layer architecture
//! (ll-store, core-store, serde-store) instead of the legacy erased_serde approach.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use structfs_core_store::{path, Error, NoCodec, Path, Reader, Record, Writer};
use structfs_serde_store::{from_value, to_value};

use crate::types::{HttpRequest, HttpResponse};

const OUTSTANDING_PREFIX: &str = "outstanding";

type RequestId = u64;

/// HTTP broker store for sync (blocking) requests (new architecture).
///
/// Write requests are queued and executed when reading from the handle path.
pub struct HttpBrokerStore {
    requests: BTreeMap<RequestId, HttpRequest>,
    next_request_id: RequestId,
    http_client: Client,
}

impl HttpBrokerStore {
    /// Create a new HTTP broker store with the given request timeout.
    pub fn new(timeout: Duration) -> Result<Self, crate::Error> {
        let http_client = Client::builder()
            .timeout(timeout)
            .build()
            .map_err(crate::Error::from)?;

        Ok(Self {
            requests: BTreeMap::new(),
            next_request_id: 0,
            http_client,
        })
    }

    /// Create with default timeout of 30 seconds.
    pub fn with_default_timeout() -> Result<Self, crate::Error> {
        Self::new(Duration::from_secs(30))
    }

    /// Execute an HTTP request and return the response.
    fn execute_request(&self, request: HttpRequest) -> Result<HttpResponse, crate::Error> {
        let method: http::Method = request.method.into();

        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let header_name =
                HeaderName::try_from(name.as_str()).map_err(|e| crate::Error::InvalidUrl {
                    message: e.to_string(),
                })?;
            let header_value =
                HeaderValue::try_from(value.as_str()).map_err(|e| crate::Error::InvalidUrl {
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
        if path.len() != 2 {
            return None;
        }
        if path[0] != OUTSTANDING_PREFIX {
            return None;
        }
        path[1].parse().ok()
    }
}

impl Reader for HttpBrokerStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let request_id = Self::parse_request_id(from).ok_or_else(|| Error::Other {
            message: format!(
                "Invalid handle path '{}'. Expected format: outstanding/{{id}}",
                from
            ),
        })?;

        let request = self
            .requests
            .remove(&request_id)
            .ok_or_else(|| Error::Other {
                message: format!("Request with ID {} not found", request_id),
            })?;

        let response = self.execute_request(request).map_err(|e| Error::Other {
            message: format!("HTTP request failed: {}", e),
        })?;

        // Convert HttpResponse to Value using serde-store
        let value = to_value(&response).map_err(|e| Error::Encode {
            format: structfs_core_store::Format::JSON,
            message: e.to_string(),
        })?;

        Ok(Some(Record::parsed(value)))
    }
}

impl Writer for HttpBrokerStore {
    fn write(&mut self, _to: &Path, data: Record) -> Result<Path, Error> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        // Convert Record to HttpRequest using serde-store
        let value = data.into_value(&NoCodec)?;
        let request: HttpRequest = from_value(value).map_err(|e| Error::Decode {
            format: structfs_core_store::Format::JSON,
            message: format!("Data must be an HttpRequest: {}", e),
        })?;

        self.requests.insert(request_id, request);

        Ok(path!(OUTSTANDING_PREFIX).join(&path!(&format!("{}", request_id))))
    }
}

/// HTTP client store for direct requests (new architecture).
///
/// Maps read/write operations to GET/POST requests.
pub struct HttpClientStore {
    client: Client,
    base_url: url::Url,
    default_headers: std::collections::HashMap<String, String>,
}

impl HttpClientStore {
    /// Create a new HTTP client store with the given base URL
    pub fn new(base_url: &str) -> Result<Self, crate::Error> {
        let base_url = url::Url::parse(base_url)?;
        let client = Client::new();

        Ok(Self {
            client,
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

    /// Execute an HTTP request and return the response
    fn execute_request(&self, request: &HttpRequest) -> Result<HttpResponse, crate::Error> {
        // Build URL
        let url = if request.path.starts_with("http://") || request.path.starts_with("https://") {
            url::Url::parse(&request.path)?
        } else {
            self.base_url.join(&request.path)?
        };

        // Build the request
        let method: http::Method = request.method.clone().into();
        let mut req_builder = self.client.request(method, url);

        // Add query parameters
        if !request.query.is_empty() {
            req_builder = req_builder.query(&request.query);
        }

        // Add default headers
        for (name, value) in &self.default_headers {
            req_builder = req_builder.header(name, value);
        }

        // Add request headers
        for (name, value) in &request.headers {
            req_builder = req_builder.header(name, value);
        }

        // Add body if present
        if let Some(body) = &request.body {
            req_builder = req_builder.json(body);
        }

        // Execute request
        let response = req_builder.send()?;

        // Build response
        let status = response.status().as_u16();
        let status_text = response
            .status()
            .canonical_reason()
            .unwrap_or("Unknown")
            .to_string();

        let mut headers = std::collections::HashMap::new();
        for (name, value) in response.headers() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.to_string(), v.to_string());
            }
        }

        // Get body as text first
        let body_text = response.text()?;

        // Try to parse as JSON
        let body = serde_json::from_str(&body_text).unwrap_or(serde_json::Value::Null);

        Ok(HttpResponse {
            status,
            status_text,
            headers,
            body,
            body_text: Some(body_text),
        })
    }

    /// Perform a GET request and return the response
    pub fn get(&self, path: &Path) -> Result<HttpResponse, crate::Error> {
        let request = HttpRequest {
            method: crate::types::Method::GET,
            path: path.components.join("/"),
            ..Default::default()
        };
        self.execute_request(&request)
    }
}

impl Reader for HttpClientStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let response = self.get(from).map_err(|e| Error::Other {
            message: e.to_string(),
        })?;

        if response.status == 404 {
            return Ok(None);
        }

        if !response.is_success() {
            return Err(Error::Other {
                message: format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            });
        }

        // Convert response body to Value
        let value = to_value(&response.body).map_err(|e| Error::Encode {
            format: structfs_core_store::Format::JSON,
            message: e.to_string(),
        })?;

        Ok(Some(Record::parsed(value)))
    }
}

impl Writer for HttpClientStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;

        // Try to interpret as HttpRequest if writing to root
        let response = if to.is_empty() {
            if let Ok(request) = from_value::<HttpRequest>(value.clone()) {
                // It's an HttpRequest, execute it directly
                self.execute_request(&request).map_err(|e| Error::Other {
                    message: e.to_string(),
                })?
            } else {
                // Not an HttpRequest, POST to root with the value as body
                let json_value = structfs_serde_store::value_to_json(value);
                let request = HttpRequest {
                    method: crate::types::Method::POST,
                    path: String::new(),
                    body: Some(json_value),
                    ..Default::default()
                };
                self.execute_request(&request).map_err(|e| Error::Other {
                    message: e.to_string(),
                })?
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
            self.execute_request(&request).map_err(|e| Error::Other {
                message: e.to_string(),
            })?
        };

        if !response.is_success() {
            return Err(Error::Other {
                message: format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            });
        }

        Ok(to.clone())
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
        assert_eq!(broker.requests.len(), 1);
    }
}
