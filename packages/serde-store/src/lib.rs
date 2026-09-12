//! Serde Integration for StructFS
//!
//! This layer provides typed access to StructFS stores via serde. It adds:
//! - `TypedReader`: Read directly into Rust types
//! - `TypedWriter`: Write Rust types directly
//! - `JsonCodec`: A codec for JSON format
//! - Value <-> serde conversions
//!
//! # Lossless values
//!
//! ```rust
//! use structfs_serde_store::{Codec, Format, Value, ValueJsonCodec};
//! let value = Value::Array(vec![Value::from(u64::MAX), Value::Bytes(vec![0, 255])]);
//! let bytes = ValueJsonCodec.encode(&value, &Format::VALUE_JSON)?;
//! let decoded = ValueJsonCodec.decode(&bytes, &Format::VALUE_JSON)?;
//! assert!(value.semantic_eq(&decoded));
//! # Ok::<(), structfs_serde_store::Error>(())
//! ```
//!
//! Plain JSON rejects bytes and non-finite floats. Use [`ValueCodec`] to select
//! a profile and [`Limits`] explicitly. [`to_value`] and [`from_value`] use the
//! structural Serde mapping directly; ambiguous null-valued options require
//! [`ExplicitOption`]. All codecs validate complete documents. Raw record
//! forwarding does not imply validation; use [`transcode`] for that contract.
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

mod cbor_profile;
mod codec;
mod convert;
mod flex_profile;
mod json_profile;
mod limits;
mod typed;
mod value_serde;

pub use codec::{
    transcode, CborCodec, FlexbuffersCodec, JsonCodec, MultiCodec, Profile, ValueCodec,
    ValueJsonCodec,
};
pub use convert::{from_value, json_to_value, to_value, value_to_json};
pub use limits::{validate_value, Limits};
pub use typed::{TypedReader, TypedWriter};
pub use value_serde::{from_value_with_limits, to_value_with_limits, ExplicitOption};

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
