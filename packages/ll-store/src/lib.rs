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
//! structfs-ll-store = { version = "0.4", features = ["async"] }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llpath_newtype_construction_and_views() {
        // from_components / components / into_components round-trip.
        let raw = vec![Bytes::from_static(b"a"), Bytes::from_static(b"b")];
        let path = LLPath::from_components(raw.clone());
        assert_eq!(path.components(), raw.as_slice());
        assert_eq!(path.clone().into_components(), raw);

        // Deref gives slice access (len / index / iter) regardless of layout.
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].as_ref(), b"a");
        assert_eq!(path.iter().count(), 2);

        // as_byte_refs borrows without copying bytes -- the shape the
        // `&[&[u8]]` read/write interface consumes.
        assert_eq!(path.as_byte_refs(), vec![b"a".as_ref(), b"b".as_ref()]);
    }

    #[test]
    fn llpath_from_iter_and_into_iter() {
        // FromIterator<Bytes>: the `.collect()` the constructors rely on.
        let path: LLPath = [Bytes::from_static(b"x"), Bytes::from_static(b"y")]
            .into_iter()
            .collect();
        assert_eq!(path.len(), 2);

        // IntoIterator (owned): the shape featherweight lowers back to the wire.
        let components: Vec<Vec<u8>> = path.into_iter().map(|b| b.to_vec()).collect();
        assert_eq!(components, vec![b"x".to_vec(), b"y".to_vec()]);
    }

    #[test]
    fn llpath_push_grows() {
        let mut path = LLPath::new();
        assert!(path.is_empty());
        path.push(Bytes::from_static(b"only"));
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].as_ref(), b"only");
    }
}
