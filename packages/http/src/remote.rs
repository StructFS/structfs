//! Remote StructFS store over HTTP.
//!
//! This module provides a client for connecting to a remote StructFS server
//! that exposes its store interface over HTTP.
//!
//! ## Protocol
//!
//! - `read(path)` → `GET /{path}` → JSON response body
//! - `write(path, data)` → `POST /{path}` with JSON body → returns result path
//!
//! ## Example
//!
//! ```ignore
//! use structfs_http::remote::RemoteStore;
//! use structfs_store::{Reader, Writer, Path};
//!
//! let mut store = RemoteStore::new("https://structfs.example.com")?;
//!
//! // Read from remote
//! let user: User = store.read_owned(&Path::parse("users/123")?)?
//!     .ok_or("User not found")?;
//!
//! // Write to remote
//! store.write(&Path::parse("users/456")?, &new_user)?;
//! ```

use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Url;
use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_store::{Error as StoreError, Path, Reader, Writer};

use crate::Error;

/// A store that connects to a remote StructFS server over HTTP.
///
/// This provides a synchronous (blocking) interface to a remote StructFS
/// server, translating read/write operations to HTTP GET/POST requests.
pub struct RemoteStore {
    client: Client,
    base_url: Url,
}

impl RemoteStore {
    /// Create a new RemoteStore connected to the given base URL.
    ///
    /// The base URL should be the root of the remote StructFS server,
    /// e.g., `https://structfs.example.com` or `http://localhost:8080`.
    pub fn new(base_url: &str) -> Result<Self, Error> {
        let base_url = Url::parse(base_url)?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = Client::builder().default_headers(headers).build()?;

        Ok(Self { client, base_url })
    }

    /// Build the full URL for a path.
    fn build_url(&self, path: &Path) -> Result<Url, Error> {
        let path_str = if path.is_empty() {
            String::new()
        } else {
            path.components.join("/")
        };

        self.base_url
            .join(&path_str)
            .map_err(|e| Error::InvalidUrl {
                message: e.to_string(),
            })
    }
}

impl Reader for RemoteStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        let url = self.build_url(from).map_err(|e| StoreError::Raw {
            message: e.to_string(),
        })?;

        let response = self.client.get(url).send().map_err(|e| StoreError::Raw {
            message: format!("HTTP request failed: {}", e),
        })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(StoreError::Raw {
                message: format!("HTTP error: {}", response.status()),
            });
        }

        let json: serde_json::Value = response.json().map_err(|e| StoreError::Raw {
            message: format!("Failed to parse response as JSON: {}", e),
        })?;

        Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
            json,
        ))))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        let url = self.build_url(from).map_err(|e| StoreError::Raw {
            message: e.to_string(),
        })?;

        let response = self.client.get(url).send().map_err(|e| StoreError::Raw {
            message: format!("HTTP request failed: {}", e),
        })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(StoreError::Raw {
                message: format!("HTTP error: {}", response.status()),
            });
        }

        let record: RecordType =
            response
                .json()
                .map_err(|e| StoreError::RecordDeserialization {
                    message: format!("Failed to deserialize response: {}", e),
                })?;

        Ok(Some(record))
    }
}

impl Writer for RemoteStore {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        let url = self.build_url(destination).map_err(|e| StoreError::Raw {
            message: e.to_string(),
        })?;

        let response = self
            .client
            .post(url)
            .json(&data)
            .send()
            .map_err(|e| StoreError::Raw {
                message: format!("HTTP request failed: {}", e),
            })?;

        if !response.status().is_success() {
            return Err(StoreError::Raw {
                message: format!("HTTP error: {}", response.status()),
            });
        }

        // Try to get the result path from the response body
        // If the server returns a path, use it; otherwise, return the destination
        if let Ok(result_path) = response.text() {
            if let Ok(path) = Path::parse(result_path.trim_matches('"')) {
                return Ok(path);
            }
        }

        Ok(destination.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_url() {
        let store = RemoteStore::new("https://example.com/api/").unwrap();

        let url = store.build_url(&Path::parse("users/123").unwrap()).unwrap();
        assert_eq!(url.as_str(), "https://example.com/api/users/123");

        let url = store.build_url(&Path::parse("").unwrap()).unwrap();
        assert_eq!(url.as_str(), "https://example.com/api/");
    }
}
