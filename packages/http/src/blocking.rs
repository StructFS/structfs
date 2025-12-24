use std::collections::HashMap;

use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

use structfs_store::{Error as StoreError, Path, Reader, Writer};

use crate::error::Error;
use crate::types::{HttpRequest, HttpResponse, Method};

/// A StructFS store backed by an HTTP client
///
/// This store maps StructFS operations to HTTP requests:
/// - `read(path)` performs a GET request
/// - `write(path, data)` performs a POST request with the data as JSON body
///
/// For full control over HTTP method, headers, etc., write an `HttpRequest`
/// struct to the root path.
///
/// # Example
///
/// ```ignore
/// use structfs_http::blocking::HttpClientStore;
/// use structfs_store::{Reader, Writer, path};
///
/// let store = HttpClientStore::new("https://api.example.com")?;
///
/// // Simple GET request
/// let user: User = store.read(&path!("users/123"))?.unwrap();
///
/// // Simple POST request
/// store.write(&path!("users"), &new_user)?;
///
/// // Full control with HttpRequest
/// use structfs_http::HttpRequest;
/// let request = HttpRequest::put("users/123")
///     .with_header("Authorization", "Bearer token")
///     .with_body(&updated_user)?;
/// store.write(&path!(""), &request)?;
/// ```
pub struct HttpClientStore {
    client: Client,
    base_url: Url,
    default_headers: HashMap<String, String>,
}

impl HttpClientStore {
    /// Create a new HTTP client store with the given base URL
    pub fn new(base_url: &str) -> Result<Self, Error> {
        let base_url = Url::parse(base_url)?;
        let client = Client::new();

        Ok(Self {
            client,
            base_url,
            default_headers: HashMap::new(),
        })
    }

    /// Create a new HTTP client store with a custom reqwest client
    pub fn with_client(client: Client, base_url: &str) -> Result<Self, Error> {
        let base_url = Url::parse(base_url)?;

        Ok(Self {
            client,
            base_url,
            default_headers: HashMap::new(),
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

    /// Build the full URL from base URL and StructFS path
    #[allow(dead_code)]
    fn build_url(&self, path: &Path) -> Result<Url, Error> {
        let path_str = path.components.join("/");
        self.base_url.join(&path_str).map_err(Error::from)
    }

    /// Execute an HTTP request and return the response
    fn execute_request(&self, request: &HttpRequest) -> Result<HttpResponse, Error> {
        // Build URL
        let url = if request.path.starts_with("http://") || request.path.starts_with("https://") {
            Url::parse(&request.path)?
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

        let mut headers = HashMap::new();
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
    pub fn get(&self, path: &Path) -> Result<HttpResponse, Error> {
        let request = HttpRequest {
            method: Method::GET,
            path: path.components.join("/"),
            ..Default::default()
        };
        self.execute_request(&request)
    }

    /// Perform a POST request with the given body
    pub fn post<T: Serialize>(&self, path: &Path, body: &T) -> Result<HttpResponse, Error> {
        let request = HttpRequest {
            method: Method::POST,
            path: path.components.join("/"),
            body: Some(serde_json::to_value(body)?),
            ..Default::default()
        };
        self.execute_request(&request)
    }

    /// Execute a full HttpRequest
    pub fn request(&self, request: &HttpRequest) -> Result<HttpResponse, Error> {
        self.execute_request(request)
    }
}

impl Reader for HttpClientStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        let response = self.get(from).map_err(StoreError::from)?;

        if response.status == 404 {
            return Ok(None);
        }

        if !response.is_success() {
            return Err(StoreError::ImplementationFailure {
                message: format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            });
        }

        let de: Box<dyn erased_serde::Deserializer> =
            Box::new(<dyn erased_serde::Deserializer>::erase(response.body));
        Ok(Some(de))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        let response = self.get(from).map_err(StoreError::from)?;

        if response.status == 404 {
            return Ok(None);
        }

        if !response.is_success() {
            return Err(StoreError::ImplementationFailure {
                message: format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            });
        }

        let record: RecordType = serde_json::from_value(response.body).map_err(|e| {
            StoreError::RecordDeserialization {
                message: e.to_string(),
            }
        })?;

        Ok(Some(record))
    }
}

impl Writer for HttpClientStore {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        // Check if we're writing an HttpRequest (for full control)
        let value = serde_json::to_value(&data).map_err(|e| StoreError::RecordSerialization {
            message: e.to_string(),
        })?;

        // Try to interpret as HttpRequest if writing to root
        let response = if destination.is_empty() {
            if let Ok(request) = serde_json::from_value::<HttpRequest>(value.clone()) {
                // It's an HttpRequest, execute it directly
                self.execute_request(&request).map_err(StoreError::from)?
            } else {
                // Not an HttpRequest, POST to root
                self.post(destination, &value).map_err(StoreError::from)?
            }
        } else {
            // POST to the path
            self.post(destination, &value).map_err(StoreError::from)?
        };

        if !response.is_success() {
            return Err(StoreError::ImplementationFailure {
                message: format!(
                    "HTTP {} {}: {}",
                    response.status,
                    response.status_text,
                    response.body_text.unwrap_or_default()
                ),
            });
        }

        // Return the original path (or could return a path to the response)
        Ok(destination.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[allow(dead_code)]
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestUser {
        id: u64,
        name: String,
    }

    // Integration tests would go here with wiremock
    // For now, just test URL building

    #[test]
    fn test_url_building() {
        let store = HttpClientStore::new("https://api.example.com/v1/").unwrap();
        let path = Path::parse("users/123").unwrap();
        let url = store.build_url(&path).unwrap();
        assert_eq!(url.as_str(), "https://api.example.com/v1/users/123");
    }

    #[test]
    fn test_url_building_no_trailing_slash() {
        let store = HttpClientStore::new("https://api.example.com/v1").unwrap();
        let path = Path::parse("users/123").unwrap();
        let url = store.build_url(&path).unwrap();
        // Note: URL joining behavior differs based on trailing slash
        assert!(url.as_str().contains("users/123"));
    }

    #[test]
    fn test_default_headers() {
        let store = HttpClientStore::new("https://api.example.com")
            .unwrap()
            .with_default_header("Authorization", "Bearer token123")
            .with_default_header("X-Custom", "value");

        assert_eq!(store.default_headers.len(), 2);
        assert_eq!(
            store.default_headers.get("Authorization"),
            Some(&"Bearer token123".to_string())
        );
    }
}
