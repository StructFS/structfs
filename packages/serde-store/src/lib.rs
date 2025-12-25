//! Serde Integration for StructFS
//!
//! This layer provides typed access to StructFS stores via serde. It adds:
//! - `TypedReader`: Read directly into Rust types
//! - `TypedWriter`: Write Rust types directly
//! - `JsonCodec`: A codec for JSON format
//! - Value <-> serde conversions
//!
//! # Example
//!
//! ```rust,ignore
//! use structfs_serde_store::{TypedReader, TypedWriter, JsonCodec};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct User {
//!     name: String,
//!     age: u32,
//! }
//!
//! fn read_user(store: &mut dyn Reader) -> Result<Option<User>, Error> {
//!     let codec = JsonCodec;
//!     store.read_as(&path!("users/123"), &codec)
//! }
//! ```
//!
//! # Async Support
//!
//! Enable the `async` feature for async trait variants:
//!
//! ```toml
//! [dependencies]
//! structfs-serde-store = { version = "0.1", features = ["async"] }
//! ```
//!
//! Then use `AsyncTypedReader` and `AsyncTypedWriter`.

pub use bytes::Bytes;

mod codec;
mod convert;
mod typed;

pub use codec::{JsonCodec, MultiCodec};
pub use convert::{from_value, to_value};
pub use typed::{TypedReader, TypedWriter};

// Re-export core types for convenience
pub use structfs_core_store::{
    Codec, Error, Format, Path, PathError, Reader, Record, Store, Value, Writer,
};

// Async support
#[cfg(feature = "async")]
mod async_typed;

#[cfg(feature = "async")]
pub use async_typed::{AsyncTypedReader, AsyncTypedWriter};

// Re-export async core types when async feature is enabled
#[cfg(feature = "async")]
pub use structfs_core_store::{
    AsyncCoreToLL, AsyncLLReader, AsyncLLStore, AsyncLLToCore, AsyncLLWriter, AsyncReader,
    AsyncStore, AsyncWriter, SyncToAsync, SyncToAsyncLL,
};
