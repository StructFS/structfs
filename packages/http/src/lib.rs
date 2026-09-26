//! # structfs-http
//!
//! HTTP client stores for StructFS.
//!
//! This crate provides StructFS Store implementations that map read/write
//! operations to HTTP requests.
//!
//! ## Store types
//!
//! | Store | Execution | Reading a handle |
//! |---|---|---|
//! | [`HttpBrokerStore`] | on read, on the calling thread | executes, then caches |
//! | [`BackgroundHttpBrokerStore`] | on a background thread, starting at write | polls status; `response/wait` parks |
//! | [`HttpClientStore`] | on read/write, on the calling thread | GET / POST against a base URL |
//!
//! Each store's own documentation carries a worked example.
//!
//! ## One execution seam
//!
//! Every blocking store sends its requests through
//! [`BlockingHttpExecutor`]; [`BlockingReqwestExecutor`] is the production
//! implementation and holds one pooled client. Substituting the seam is how
//! tests run without a network, and how an embedder swaps in its own
//! transport. The async, streaming counterpart is
//! [`streaming::AsyncHttpExecutor`], behind the `streaming` feature.
//!
//! ## Runtime constraints
//!
//! Building a blocking reqwest client **panics inside a Tokio runtime**, so
//! every store that uses the default executor must be constructed off the
//! runtime. [`BackgroundHttpBrokerStore`] additionally *owns* a small
//! runtime and drives its handle store with `block_on`, so it must not be
//! called from inside another runtime either. Both constraints are repeated
//! on the types themselves.
//!
//! ## Errors
//!
//! HTTP failures reach store callers as typed
//! [`structfs_core_store::Error`] variants — a timeout is
//! `DeadlineExceeded`, a 401 is `PermissionDenied`, a malformed URL is
//! `InvalidArgument`. See [`error`] for the full mapping.
//!
//! ## Features
//!
//! - `blocking` (default): the three stores above, over reqwest's blocking client.
//! - `streaming`: [`streaming::AsyncReqwestExecutor`], an incremental
//!   pull-based response body.
//!
//! With `default-features = false` only the portable request/response/status
//! types, [`sse`] and [`error`] remain — no native HTTP dependency.

pub mod error;
#[cfg(feature = "blocking")]
pub mod executor;
pub mod handle;
pub mod sse;
#[cfg(feature = "streaming")]
pub mod streaming;
pub mod types;

#[cfg(feature = "blocking")]
mod background_broker;
#[cfg(feature = "blocking")]
mod broker_common;
#[cfg(feature = "blocking")]
mod client_store;
#[cfg(feature = "blocking")]
mod handle_broker;
#[cfg(feature = "blocking")]
mod sync_broker;

// Re-export main types
pub use error::Error;
#[cfg(feature = "blocking")]
pub use executor::{BlockingHttpExecutor, BlockingReqwestExecutor};
pub use handle::{RequestState, RequestStatus};
pub use types::{HttpRequest, HttpResponse, Method};

// Re-export stores
#[cfg(feature = "blocking")]
pub use background_broker::BackgroundHttpBrokerStore;
#[cfg(feature = "blocking")]
pub use client_store::HttpClientStore;
#[cfg(feature = "blocking")]
pub use sync_broker::HttpBrokerStore;

#[cfg(all(feature = "blocking", target_arch = "wasm32"))]
compile_error!("structfs-http stores require a native target; use default-features = false for portable HTTP types");
