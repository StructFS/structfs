//! Server protocol for StructFS stores.
//!
//! The server protocol enables stores to act as "servers" that can read from
//! other stores to fulfill requests. This is implemented purely through
//! StructFS read/write operations - no special Rust traits needed.
//!
//! ## How It Works
//!
//! Rather than giving servers special access to read arbitrary paths, we use
//! **mounting** to make relevant paths visible to the server:
//!
//! 1. Stores register what paths they expose (e.g., docs at "docs")
//! 2. When a server needs access to other stores, those paths are mounted
//!    into the server's namespace
//! 3. The server reads from its own paths (which happen to be mounts)
//!
//! ## Example: Help Store
//!
//! ```text
//! # Sys store provides docs at /ctx/sys/docs
//! # Help store wants to show sys docs when user reads /ctx/help/sys
//!
//! # Solution: Mount sys docs into help store's namespace
//! /ctx/help/
//!     stores/
//!         sys/     <- mounted from /ctx/sys/docs
//!             env
//!             time
//!             ...
//!
//! # Now help store can read from stores/sys to get sys docs
//! ```
//!
//! ## Store Registration
//!
//! Stores can provide a `StoreRegistration` to declare:
//! - `docs_path`: Where their documentation lives (e.g., "docs")
//!
//! The mounting system uses this info to wire up cross-store access.

use serde::{Deserialize, Serialize};

/// Registration info that a store provides when it wants to participate in protocols.
///
/// This is written as JSON to a registration path, enabling discovery by the
/// mounting system.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StoreRegistration {
    /// If the store provides documentation, the path relative to its root
    /// where docs can be read (e.g., "docs")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_path: Option<String>,
}

impl StoreRegistration {
    /// Create a registration with a docs path
    pub fn with_docs(docs_path: impl Into<String>) -> Self {
        Self {
            docs_path: Some(docs_path.into()),
        }
    }
}
