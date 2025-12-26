//! Adapter layer between legacy structfs-store and new core-store architecture.
//!
//! This crate provides bidirectional adapters allowing:
//! - Legacy stores to be used with new core-store traits
//! - New stores to be used with legacy store traits
//!
//! # Usage
//!
//! ```rust,ignore
//! use structfs_legacy_adapter::{LegacyToNew, NewToLegacy};
//!
//! // Wrap a legacy store to use with new traits
//! let legacy_store = MyLegacyStore::new();
//! let mut new_store = LegacyToNew::new(legacy_store);
//! let record = new_store.read(&path!("foo/bar"))?;
//!
//! // Wrap a new store to use with legacy traits
//! let new_store = MyNewStore::new();
//! let mut legacy_store = NewToLegacy::new(new_store);
//! let value: MyType = legacy_store.read_owned(&path!("foo/bar"))?;
//! ```

mod error;
mod legacy_to_new;
mod new_to_legacy;
mod path_convert;

pub use error::Error;
pub use legacy_to_new::LegacyToNew;
pub use new_to_legacy::NewToLegacy;
pub use path_convert::{core_path_to_legacy, legacy_path_to_core};

// Re-export key types for convenience
pub use structfs_core_store::{
    path as new_path, Path as NewPath, Reader as NewReader, Record, Store as NewStore, Value,
    Writer as NewWriter,
};
pub use structfs_store::{
    path as legacy_path, Path as LegacyPath, Reader as LegacyReader, Store as LegacyStore,
    Writer as LegacyWriter,
};
