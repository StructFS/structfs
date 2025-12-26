//! # structfs-sys
//!
//! OS primitives exposed through StructFS paths.
//!
//! This crate provides standard OS functionality through the StructFS
//! read/write interface, designed for environments where programs interact
//! with the OS exclusively through StructFS operations.
//!
//! ## Path Namespace
//!
//! ```text
//! /sys/
//!     env/          # Environment variables
//!     time/         # Clocks and sleep
//!     random/       # Random number generation
//!     proc/         # Process information
//!     fs/           # Filesystem operations
//!     docs/         # Documentation for this store
//! ```
//!
//! ## Example
//!
//! ```rust,ignore
//! use structfs_sys::SysStore;
//! use structfs_store::{Reader, Path};
//!
//! let mut store = SysStore::new();
//!
//! // Read environment variable
//! let home: Option<String> = store.read_owned(&Path::parse("env/HOME").unwrap()).unwrap();
//!
//! // Get current time
//! let now: Option<String> = store.read_owned(&Path::parse("time/now").unwrap()).unwrap();
//!
//! // Get documentation
//! let docs: Option<serde_json::Value> = store.read_owned(&Path::parse("docs").unwrap()).unwrap();
//! ```

// Legacy implementations (to be deprecated)
pub mod docs;
pub mod env;
pub mod fs;
pub mod proc;
pub mod random;
pub mod time;

// New core-store based implementations
pub mod core;

use serde::de::DeserializeOwned;
use serde::Serialize;
use structfs_store::{Error, OverlayStore, Path, Reader, Writer};

pub use docs::DocsStore;
pub use env::EnvStore;
pub use fs::FsStore;
pub use proc::ProcStore;
pub use random::RandomStore;
pub use time::TimeStore;

/// The main system store that composes all OS primitive stores.
///
/// Mount this at `/sys` to expose OS functionality through StructFS paths.
pub struct SysStore<'a> {
    inner: OverlayStore<'a>,
}

impl<'a> SysStore<'a> {
    /// Create a new system store with all sub-stores mounted.
    pub fn new() -> Self {
        let mut overlay = OverlayStore::default();

        overlay
            .add_layer(Path::parse("env").unwrap(), EnvStore::new())
            .unwrap();
        overlay
            .add_layer(Path::parse("time").unwrap(), TimeStore::new())
            .unwrap();
        overlay
            .add_layer(Path::parse("random").unwrap(), RandomStore::new())
            .unwrap();
        overlay
            .add_layer(Path::parse("proc").unwrap(), ProcStore::new())
            .unwrap();
        overlay
            .add_layer(Path::parse("fs").unwrap(), FsStore::new())
            .unwrap();
        overlay
            .add_layer(Path::parse("docs").unwrap(), DocsStore::new())
            .unwrap();

        Self { inner: overlay }
    }
}

impl Default for SysStore<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for SysStore<'_> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
        self.inner.read_to_deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error> {
        self.inner.read_owned(from)
    }
}

impl Writer for SysStore<'_> {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        self.inner.write(destination, data)
    }
}
