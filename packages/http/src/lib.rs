//! # structfs-http
//!
//! HTTP client and server stores for StructFS.
//!
//! This crate provides StructFS Store implementations that map read/write
//! operations to HTTP requests. It follows the StructFS philosophy of
//! "synchronous interface, asynchronous effects".
//!
//! ## Two Client Modes
//!
//! ### 1. Blocking Client (`HttpClientStore`)
//!
//! Synchronous HTTP - calls block until the response arrives:
//!
//! ```ignore
//! use structfs_http::blocking::HttpClientStore;
//! use structfs_store::{Reader, Writer, Path};
//!
//! let mut client = HttpClientStore::new("https://api.example.com")?;
//!
//! // GET request via read_owned() - blocks until complete
//! let user: User = client.read_owned(&Path::parse("users/123")?)?.unwrap();
//!
//! // POST request via write() - blocks until complete
//! client.write(&Path::parse("users")?, &new_user)?;
//! ```
//!
//! ### 2. Async Client (`AsyncHttpClientStore`)
//!
//! Non-blocking HTTP via the handle pattern - requests return immediately
//! with a handle path that can be queried for status:
//!
//! ```ignore
//! use structfs_http::async_client::AsyncHttpClientStore;
//! use structfs_http::{HttpRequest, RequestStatus};
//! use structfs_store::{Reader, Writer, Path};
//!
//! let mut client = AsyncHttpClientStore::new("https://api.example.com")?;
//!
//! // Initiate request - returns immediately with handle path
//! let handle = client.write(&Path::parse("")?, &HttpRequest::get("users/123"))?;
//! // handle = "handles/0000000000000000"
//!
//! // Query status (non-blocking)
//! let status: RequestStatus = client.read_owned(&handle)?.unwrap();
//! // status.state = Pending | Complete | Failed
//!
//! // Block until complete
//! client.write(&handle.join(&Path::parse("await")?), &())?;
//!
//! // Read the response
//! let response: HttpResponse = client.read_owned(
//!     &handle.join(&Path::parse("response")?)
//! )?.unwrap();
//! ```
//!
//! ## Full Control with HttpRequest
//!
//! For requests that need custom methods, headers, or query parameters:
//!
//! ```ignore
//! use structfs_http::HttpRequest;
//!
//! let request = HttpRequest::put("users/123")
//!     .with_header("Authorization", "Bearer token")
//!     .with_query("version", "2")
//!     .with_body(&user)?;
//!
//! // Write the request to execute it
//! client.write(&Path::parse("")?, &request)?;
//! ```
//!
//! ## Async Client Path Reference
//!
//! | Path | Operation | Description |
//! |------|-----------|-------------|
//! | `""` | write(HttpRequest) | Initiate request, returns handle path |
//! | `handles/{id}` | read | Get RequestStatus |
//! | `handles/{id}/response` | read | Get HttpResponse (None if pending) |
//! | `handles/{id}/await` | write | Block until complete |
//! | `handles/{id}/await_timeout` | write(ms) | Block with timeout |

pub mod error;
pub mod handle;
pub mod types;

#[cfg(feature = "blocking")]
pub mod blocking;

#[cfg(feature = "blocking")]
pub mod broker;

#[cfg(feature = "blocking")]
pub mod remote;

#[cfg(feature = "async")]
pub mod async_client;

// Re-export main types
pub use error::Error;
pub use handle::{RequestState, RequestStatus};
pub use types::{HttpRequest, HttpResponse, Method};

#[cfg(feature = "blocking")]
pub use blocking::HttpClientStore;

#[cfg(feature = "blocking")]
pub use broker::HttpBrokerStore;

#[cfg(feature = "blocking")]
pub use remote::RemoteStore;

#[cfg(feature = "async")]
pub use async_client::AsyncHttpClientStore;
