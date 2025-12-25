//! LLStructFS: Low-Level StructFS Store Traits
//!
//! This is the narrow waist of the StructFS stack. Everything at this level is
//! pure bytes - no path validation, no value semantics, no format interpretation.
//!
//! Use this layer for:
//! - WASM/FFI boundaries where you're marshalling raw memory
//! - Wire protocols where you're moving bytes without inspection
//! - Zero-copy forwarding proxies
//! - Any transport that shouldn't pay parsing costs
//!
//! # Example
//!
//! ```rust
//! use structfs_ll_store::{LLReader, LLWriter, LLError};
//! use bytes::Bytes;
//!
//! struct InMemoryLLStore {
//!     data: std::collections::HashMap<Vec<Vec<u8>>, Bytes>,
//! }
//!
//! impl LLReader for InMemoryLLStore {
//!     fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
//!         let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
//!         Ok(self.data.get(&key).cloned())
//!     }
//! }
//! ```
//!
//! # Async Support
//!
//! Enable the `async` feature for async trait variants:
//!
//! ```toml
//! [dependencies]
//! structfs-ll-store = { version = "0.1", features = ["async"] }
//! ```
//!
//! Then use `AsyncLLReader`, `AsyncLLWriter`, and `AsyncLLStore`.

pub use bytes::Bytes;

mod error;
mod traits;

pub use error::LLError;
pub use traits::{LLPath, LLReader, LLStore, LLWriter};

#[cfg(feature = "async")]
mod async_traits;

#[cfg(feature = "async")]
pub use async_traits::{AsyncLLReader, AsyncLLStore, AsyncLLWriter, SyncToAsyncLL};

/// Convenience function to create an owned path from byte slices.
pub fn ll_path(components: &[&[u8]]) -> LLPath {
    components
        .iter()
        .map(|c| Bytes::copy_from_slice(c))
        .collect()
}

/// Convenience function to create an owned path from string slices.
pub fn ll_path_from_strs(components: &[&str]) -> LLPath {
    components
        .iter()
        .map(|s| Bytes::copy_from_slice(s.as_bytes()))
        .collect()
}
