//! # structfs-http
//!
//! HTTP client stores for StructFS.
//!
//! This crate provides StructFS Store implementations that map read/write
//! operations to HTTP requests.
//!
//! ## Store Types
//!
//! ### HttpBrokerStore (Sync)
//!
//! Blocking HTTP broker - write queues a request, read from handle executes it:
//!
//! ```ignore
//! use structfs_http::{HttpBrokerStore, HttpRequest};
//! use structfs_core_store::{Reader, Writer, Path};
//!
//! let mut broker = HttpBrokerStore::with_default_timeout()?;
//!
//! // Queue a request
//! let handle = broker.write(&Path::parse("")?, Record::parsed(to_value(&HttpRequest::get("https://example.com"))?))?;
//!
//! // Execute and get response (blocks)
//! let response = broker.read(&handle)?;
//! ```
//!
//! ### AsyncHttpBrokerStore
//!
//! Non-blocking HTTP broker - requests execute in background threads:
//!
//! ```ignore
//! use structfs_http::{AsyncHttpBrokerStore, HttpRequest};
//!
//! let mut broker = AsyncHttpBrokerStore::with_default_timeout()?;
//!
//! // Queue request (starts executing immediately in background)
//! let handle = broker.write(&Path::parse("")?, Record::parsed(to_value(&HttpRequest::get("https://example.com"))?))?;
//!
//! // Check status
//! let status = broker.read(&handle)?;
//!
//! // Get response when ready
//! let response = broker.read(&handle.join(&Path::parse("response")?))?;
//! ```
//!
//! ### HttpClientStore
//!
//! Direct HTTP client with a base URL:
//!
//! ```ignore
//! use structfs_http::HttpClientStore;
//!
//! let mut client = HttpClientStore::new("https://api.example.com")?;
//!
//! // GET request via read
//! let data = client.read(&Path::parse("users/123")?)?;
//!
//! // POST request via write
//! client.write(&Path::parse("users")?, data)?;
//! ```

pub mod error;
#[cfg(feature = "blocking")]
pub mod executor;
pub mod handle;
#[cfg(feature = "blocking")]
mod handle_broker;
pub mod types;

#[cfg(feature = "blocking")]
mod core;

// Re-export main types
pub use error::Error;
#[cfg(feature = "blocking")]
pub use executor::{HttpExecutor, ReqwestExecutor};
pub use handle::{RequestState, RequestStatus};
pub use types::{HttpRequest, HttpResponse, Method};

// Re-export stores
#[cfg(feature = "blocking")]
pub use crate::core::{AsyncHttpBrokerStore, HttpBrokerStore, HttpClientStore};

#[cfg(all(feature = "blocking", target_arch = "wasm32"))]
compile_error!("structfs-http stores require a native target; use default-features = false for portable HTTP types");
