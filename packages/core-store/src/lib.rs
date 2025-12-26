//! Core StructFS: Semantic Store Layer
//!
//! This layer adds meaning to the raw bytes of LLStructFS:
//! - `Path`: Validated path with Unicode identifier components
//! - `Value`: Parsed tree structure (the "struct" in StructFS)
//! - `Record`: Either raw bytes or parsed Value
//! - `Format`: Hint about wire format for codecs
//!
//! Use this layer for:
//! - Application routing based on validated paths
//! - Format-aware caching and transformation
//! - Working with structured data
//!
//! # Example
//!
//! ```rust
//! use structfs_core_store::{Reader, Writer, Record, Path, path};
//!
//! fn read_user(store: &mut dyn Reader) -> Result<Option<Record>, structfs_core_store::Error> {
//!     store.read(&path!("users/123"))
//! }
//! ```

pub use bytes::Bytes;

mod bridge;
mod error;
mod format;
mod lazy_record;
pub mod mount_store;
pub mod overlay_store;
mod path;
mod record;
mod traits;
mod value;

pub use bridge::{CoreToLL, LLToCore};
pub use error::{CodecOperation, Error};
pub use format::Format;
pub use lazy_record::LazyRecord;
pub use path::{Path, PathError};
pub use record::Record;
pub use traits::{Codec, NoCodec, Reader, Store, Writer};
pub use value::Value;

// Re-export LL types for convenience
pub use structfs_ll_store::{LLError, LLPath, LLReader, LLStore, LLWriter};

// Async support
#[cfg(feature = "async")]
mod async_traits;

#[cfg(feature = "async")]
mod async_bridge;

#[cfg(feature = "async")]
pub use async_traits::{AsyncReader, AsyncStore, AsyncWriter, SyncToAsync};

#[cfg(feature = "async")]
pub use async_bridge::{AsyncCoreToLL, AsyncLLToCore};

// Re-export async LL types when async feature is enabled
#[cfg(feature = "async")]
pub use structfs_ll_store::{AsyncLLReader, AsyncLLStore, AsyncLLWriter, SyncToAsyncLL};
