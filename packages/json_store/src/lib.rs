//! # structfs-json-store
//!
//! JSON-based StructFS store implementations.
//!
//! This crate provides in-memory store implementations for StructFS.

pub mod append_log;
pub mod in_memory;
pub mod persist;
pub mod value_utils;

pub use append_log::{AppendBacking, JsonlFileBacking, LogStore, MemoryAppendBacking};
pub use in_memory::InMemoryStore;
pub use persist::{BackedStore, Backing, JsonFileBacking};
pub use structfs_core_store::{path, Error, Path, Reader, Record, Value, Writer};
