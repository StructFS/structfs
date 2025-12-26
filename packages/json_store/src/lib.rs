//! # structfs-json-store
//!
//! JSON-based StructFS store implementations.
//!
//! This crate provides in-memory store implementations for StructFS.

pub mod in_memory;
pub mod value_utils;

pub use in_memory::InMemoryStore;
pub use structfs_core_store::{path, Error, Path, Reader, Record, Value, Writer};
