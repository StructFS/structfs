//! Serde Integration for StructFS
//!
//! This layer provides typed access to StructFS stores via serde. It adds:
//! - [`TypedReader`] / [`TypedWriter`]: read and write Rust types directly
//! - Bounded, versioned wire codecs: [`JsonCodec`], [`ValueJsonCodec`],
//!   [`CborCodec`], [`FlexbuffersCodec`], and [`ValueCodec`] for explicit
//!   profile and [`Limits`] selection
//! - [`Value`] ⇄ serde conversions ([`to_value`], [`from_value`]) and
//!   [`Value`] ⇄ `serde_json::Value` conversions ([`value_to_json`],
//!   [`json_to_value`])
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
//! # Typed access
//!
//! The typed surface is one operation matrix in three flavours, so a call site
//! keeps its shape when it moves between them:
//!
//! | Operation | Sync | Async (`async` feature) | Detached (`async` feature) |
//! |---|---|---|---|
//! | Read, parsing raw records with a codec | `read_as` | `read_as_async` | `read_as_detached` |
//! | Read, parsed records only | `read_typed` | `read_typed_async` | `read_typed_detached` |
//! | Write | `write_typed` | `write_typed_async` | `write_typed_detached` |
//! | Read children, typed | `read_children_typed` | — | — |
//!
//! Only the sync flavour enumerates children, because only
//! [`Reader`] has `read_children`; the async and detached core traits have no
//! child enumeration for a typed wrapper to sit on.
//!
//! There is no codec-taking write in any flavour. A typed write always
//! produces a parsed record; how (and whether) it is serialized is the
//! store's decision, not the caller's.
//!
//! ## Passing a codec
//!
//! Every codec-taking method takes an **`Arc<dyn Codec>`**. A detached
//! operation outlives the call that created it, so it cannot borrow one; and
//! since `Codec: Send + Sync`, an `Arc` is equally usable from the sync and
//! async flavours. One shape means one codec handle can be built once and
//! handed to all three:
//!
//! ```rust
//! use std::sync::Arc;
//! use structfs_serde_store::{Codec, JsonCodec};
//!
//! let codec: Arc<dyn Codec> = Arc::new(JsonCodec);
//! // codec.clone() goes to read_as, read_as_async and read_as_detached alike
//! ```
//!
//! # Async Support
//!
//! Enable the `async` feature for async trait variants:
//!
//! ```toml
//! [dependencies]
//! structfs-serde-store = { version = "0.4", features = ["async"] }
//! ```
//!
//! Use `AsyncTypedReader`/`AsyncTypedWriter` for borrowing futures, or
//! `DetachedTypedReader`/`DetachedTypedWriter` for independent operations.

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
    transcode, CborCodec, CodecProfile, FlexbuffersCodec, JsonCodec, MultiCodec, ValueCodec,
    ValueJsonCodec,
};
pub use convert::{from_value, json_to_value, to_value, value_to_json};
pub use limits::Limits;
pub use typed::{TypedReader, TypedWriter};
pub use value_serde::{from_value_with_limits, to_value_with_limits, ExplicitOption};

// Core types re-exported for convenience, so a caller that only needs typed
// access does not also have to name `structfs-core-store`. Deliberately
// limited to what the codec and typed surfaces take or return — the byte
// layer and the routing traits belong to core-store. `path!` comes along with
// `Path` so such callers can build literal paths without a runtime parse.
pub use structfs_core_store::{path, Codec, Error, Format, Path, Reader, Record, Value, Writer};

// Async support
#[cfg(feature = "async")]
mod async_typed;

#[cfg(feature = "async")]
pub use async_typed::{AsyncTypedReader, AsyncTypedWriter};

#[cfg(feature = "async")]
pub use structfs_core_store::{
    AsyncReader, AsyncWriter, DetachedFuture, DetachedReader, DetachedWriter,
};

#[cfg(feature = "async")]
mod detached_typed;
#[cfg(feature = "async")]
pub use detached_typed::{DetachedTypedReader, DetachedTypedWriter};
