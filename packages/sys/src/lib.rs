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

mod docs;
mod env;
mod fs;
mod proc;
mod random;
mod time;

pub use docs::DocsStore;
pub use env::EnvStore;
pub use fs::{FsStore, OpenMode};
pub use proc::ProcStore;
pub use random::RandomStore;
pub use time::TimeStore;

use structfs_core_store::{overlay_store::OverlayStore, Error, Path, Reader, Record, Writer};

/// The main system store that composes all OS primitive stores.
///
/// Mount this at `/sys` to expose OS functionality through StructFS paths.
pub struct SysStore {
    inner: OverlayStore,
}

impl SysStore {
    /// Create a new system store with all sub-stores mounted.
    pub fn new() -> Self {
        let mut overlay = OverlayStore::new();

        overlay.mount(Path::parse("env").unwrap(), Box::new(EnvStore::new()));
        overlay.mount(Path::parse("time").unwrap(), Box::new(TimeStore::new()));
        overlay.mount(Path::parse("random").unwrap(), Box::new(RandomStore::new()));
        overlay.mount(Path::parse("proc").unwrap(), Box::new(ProcStore::new()));
        overlay.mount(Path::parse("fs").unwrap(), Box::new(FsStore::new()));
        overlay.mount(Path::parse("docs").unwrap(), Box::new(DocsStore::new()));

        Self { inner: overlay }
    }
}

impl Default for SysStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for SysStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.inner.read(from)
    }
}

impl Writer for SysStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.inner.write(to, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, NoCodec, Value};

    #[test]
    fn sys_store_read_env() {
        std::env::set_var("STRUCTFS_SYS_TEST", "value");
        let mut store = SysStore::new();
        let record = store
            .read(&path!("env/STRUCTFS_SYS_TEST"))
            .unwrap()
            .unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("value".to_string()));
        std::env::remove_var("STRUCTFS_SYS_TEST");
    }

    #[test]
    fn sys_store_read_time() {
        let mut store = SysStore::new();
        let record = store.read(&path!("time/now")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert!(s.contains("T")),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn sys_store_read_random() {
        let mut store = SysStore::new();
        let record = store.read(&path!("random/uuid")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert_eq!(s.len(), 36),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn sys_store_read_proc() {
        let mut store = SysStore::new();
        let record = store.read(&path!("proc/self/pid")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Integer(pid) => assert_eq!(pid, std::process::id() as i64),
            _ => panic!("Expected integer"),
        }
    }

    #[test]
    fn sys_store_read_docs() {
        let mut store = SysStore::new();
        let record = store.read(&path!("docs")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => assert!(map.contains_key("title")),
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn sys_store_default() {
        let mut store: SysStore = Default::default();
        // Just verify it works
        let record = store.read(&path!("time/now")).unwrap();
        assert!(record.is_some());
    }

    #[test]
    fn sys_store_write() {
        let mut store = SysStore::new();
        // Write to env
        store
            .write(
                &path!("env/STRUCTFS_SYS_WRITE_TEST"),
                Record::parsed(Value::String("written".into())),
            )
            .unwrap();
        let record = store
            .read(&path!("env/STRUCTFS_SYS_WRITE_TEST"))
            .unwrap()
            .unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("written".into()));
        // Cleanup
        store
            .write(
                &path!("env/STRUCTFS_SYS_WRITE_TEST"),
                Record::parsed(Value::Null),
            )
            .unwrap();
    }
}
