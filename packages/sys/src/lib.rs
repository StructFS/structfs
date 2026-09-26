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
//!     env/          # Environment variables (writes go to an in-process overlay)
//!     time/         # Clocks and sleep
//!     random/       # Random number generation
//!     proc/         # Process information
//!     fs/           # Filesystem operations
//!     docs/         # Documentation for this store
//! ```
//!
//! ## Exposure
//!
//! [`SysStore::new`] mounts an unrooted [`FsStore`]: whoever can write to
//! the store can open, create, and delete any file the process can reach.
//! Use [`SysStore::rooted`] (or [`SysStore::with_fs`] with
//! [`FsStore::rooted`]) to confine `fs` to one directory tree.

mod docs;
mod env;
mod fs;
mod proc;
mod random;
mod time;

pub use docs::DocsStore;
pub use env::EnvStore;
pub use fs::{
    ContentEncoding, FsStore, OpenMode, DEFAULT_MAX_HANDLES, DEFAULT_MAX_READ_LEN,
    DEFAULT_MAX_RESULTS,
};
pub use proc::ProcStore;
pub use random::{RandomStore, MAX_RANDOM_BYTES};
pub use time::{TimeStore, MAX_SLEEP};

use std::collections::BTreeMap;

use structfs_core_store::{
    overlay_store::OverlayStore, path, Error, NoCodec, Path, Reader, Record, Value, Writer,
};

/// The sub-stores of [`SysStore`], in listing order.
const SUBSTORES: [&str; 6] = ["env", "time", "random", "proc", "fs", "docs"];

/// The main system store that composes all OS primitive stores.
///
/// Mount this at `/sys` to expose OS functionality through StructFS paths.
/// Reading the root returns a map of each sub-store's root.
pub struct SysStore {
    inner: OverlayStore,
}

impl SysStore {
    /// A system store whose `fs` has whole-filesystem access (see
    /// [`FsStore::new`]). Prefer [`rooted`](Self::rooted) when the store is
    /// reachable by anyone who should not have that.
    pub fn new() -> Self {
        Self::with_fs(FsStore::new())
    }

    /// A system store whose `fs` is confined to the directory `root` (see
    /// [`FsStore::rooted`]).
    pub fn rooted(root: impl Into<std::path::PathBuf>) -> Result<Self, Error> {
        Ok(Self::with_fs(FsStore::rooted(root)?))
    }

    /// A system store using `fs` for its `fs` sub-store.
    pub fn with_fs(fs: FsStore) -> Self {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("env"), Box::new(EnvStore::new()));
        overlay.mount(path!("time"), Box::new(TimeStore::new()));
        overlay.mount(path!("random"), Box::new(RandomStore::new()));
        overlay.mount(path!("proc"), Box::new(ProcStore::new()));
        overlay.mount(path!("fs"), Box::new(fs));
        overlay.mount(path!("docs"), Box::new(DocsStore::new()));
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
        if !from.is_empty() {
            return self.inner.read(from);
        }
        let mut children = BTreeMap::new();
        for name in SUBSTORES {
            let child = Path::parse(name)?;
            if let Some(record) = self.inner.read(&child)? {
                children.insert(name.to_string(), record.into_value(&NoCodec)?);
            }
        }
        Ok(Some(Record::parsed(Value::Map(children))))
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        if from.is_empty() {
            let mut names: Vec<String> = SUBSTORES.iter().map(|s| s.to_string()).collect();
            names.sort();
            return Ok(Some(names));
        }
        self.inner.read_children(from)
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

    fn read(store: &mut SysStore, at: &Path) -> Option<Value> {
        store
            .read(at)
            .unwrap()
            .map(|r| r.into_value(&NoCodec).unwrap())
    }

    #[test]
    fn reads_route_to_substores() {
        let mut store = SysStore::new();
        // Cargo sets CARGO_PKG_NAME for test processes.
        assert_eq!(
            read(&mut store, &path!("env/CARGO_PKG_NAME")),
            Some(Value::String("structfs-sys".into()))
        );
        assert!(
            matches!(read(&mut store, &path!("time/now")), Some(Value::String(s)) if s.contains('T'))
        );
        assert!(
            matches!(read(&mut store, &path!("random/uuid")), Some(Value::String(s)) if s.len() == 36)
        );
        assert_eq!(
            read(&mut store, &path!("proc/self/pid")),
            Some(Value::Integer(std::process::id() as i64))
        );
        assert!(
            matches!(read(&mut store, &path!("docs")), Some(Value::Map(m)) if m.get("title") == Some(&Value::String("System Primitives".into())))
        );
    }

    #[test]
    fn root_lists_substores() {
        let mut store = SysStore::default();
        let Some(Value::Map(root)) = read(&mut store, &path!("")) else {
            panic!("expected a map of sub-stores")
        };
        let mut keys: Vec<_> = root.keys().cloned().collect();
        let mut expected: Vec<_> = SUBSTORES.iter().map(|s| s.to_string()).collect();
        keys.sort();
        expected.sort();
        assert_eq!(keys, expected);
        assert_eq!(store.read_children(&path!("")).unwrap(), Some(expected));
        assert!(store
            .read_children(&path!("time"))
            .unwrap()
            .unwrap()
            .contains(&"now".to_string()));
    }

    #[test]
    fn env_writes_do_not_touch_the_process_environment() {
        let mut store = SysStore::new();
        let var = path!("STRUCTFS_SYS_OVERLAY_TEST");
        let at = path!("env").join(&var);
        store
            .write(&at, Record::parsed(Value::String("written".into())))
            .unwrap();
        assert_eq!(read(&mut store, &at), Some(Value::String("written".into())));
        assert!(std::env::var("STRUCTFS_SYS_OVERLAY_TEST").is_err());
        store.write(&at, Record::parsed(Value::Null)).unwrap();
        assert_eq!(read(&mut store, &at), None);
    }

    #[test]
    fn rooted_sys_confines_fs() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut store = SysStore::rooted(dir.path()).unwrap();
        let open = |path: &str| {
            Record::parsed(Value::Map(BTreeMap::from([
                ("path".to_string(), Value::String(path.into())),
                ("mode".to_string(), Value::String("write".into())),
            ])))
        };
        store.write(&path!("fs/open"), open("inside.txt")).unwrap();
        assert!(dir.path().join("inside.txt").exists());
        assert!(matches!(
            store.write(&path!("fs/open"), open("../outside.txt")),
            Err(Error::PermissionDenied { .. })
        ));
    }
}
