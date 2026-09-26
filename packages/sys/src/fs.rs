//! Filesystem operations store.
//!
//! Actions are writes of a request map to an action path (`open`, `stat`,
//! `readdir`, `mkdir`, `rmdir`, `unlink`, `rename`). `open` returns a
//! `handles/{id}` path for handle-based I/O; `stat` and `readdir` return a
//! `results/{id}` path to read the answer from. See the crate README for the
//! full contract.
//!
//! # Confinement
//!
//! [`FsStore::new`] accepts any OS path the process can reach and follows
//! symlinks: it is whole-filesystem access. [`FsStore::rooted`] confines
//! every path to one directory tree, rejecting (after symlink resolution)
//! anything outside it with `PermissionDenied`.

mod encoding;
mod handles;
mod meta;
mod ops;

use std::collections::BTreeMap;
use std::path::PathBuf;

use collection_literals::btree;
use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Reference, Value, Writer};

pub use encoding::{ContentEncoding, OpenMode};
use handles::{parse_handle_operation, FileHandle};

/// Default cap on the bytes a single handle read may return (16 MiB).
pub const DEFAULT_MAX_READ_LEN: u64 = 16 * 1024 * 1024;

/// Default number of `stat`/`readdir` answers retained under `results`.
pub const DEFAULT_MAX_RESULTS: usize = 256;

/// Default number of handles one store may hold open at once.
pub const DEFAULT_MAX_HANDLES: usize = 256;

/// Store for filesystem operations.
pub struct FsStore {
    /// Open files, keyed by per-store id (listed in id order).
    handles: BTreeMap<u64, FileHandle>,
    /// `stat`/`readdir` answers, keyed by per-store id.
    results: BTreeMap<u64, Value>,
    /// Next id for a handle or result; ids are never reused within a store.
    next_id: u64,
    /// Canonical confinement root; `None` for whole-filesystem access.
    root: Option<PathBuf>,
    max_read_len: u64,
    max_results: usize,
    max_handles: usize,
}

impl FsStore {
    /// An fs store with access to the whole filesystem the process can
    /// reach. Symlinks are followed; nothing is confined. Use
    /// [`rooted`](Self::rooted) to expose only one directory tree.
    pub fn new() -> Self {
        Self {
            handles: BTreeMap::new(),
            results: BTreeMap::new(),
            next_id: 0,
            root: None,
            max_read_len: DEFAULT_MAX_READ_LEN,
            max_results: DEFAULT_MAX_RESULTS,
            max_handles: DEFAULT_MAX_HANDLES,
        }
    }

    /// An fs store confined to the directory `root`.
    ///
    /// `root` is canonicalized (it must exist). Relative request paths are
    /// resolved against it; absolute ones are accepted only if they lie
    /// inside it. Every path is checked after symlink resolution, so a
    /// symlink pointing out of the tree is `PermissionDenied`, as is a
    /// dangling symlink. Checks happen at request time; a concurrent
    /// process that swaps directories for symlinks between the check and
    /// the operation is outside this guarantee.
    pub fn rooted(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let root = std::fs::canonicalize(root.into())?;
        if !root.is_dir() {
            return Err(Error::invalid_argument(format!(
                "fs root '{}' is not a directory",
                root.display()
            )));
        }
        Ok(Self {
            root: Some(root),
            ..Self::new()
        })
    }

    /// Cap the bytes a single handle read may return (default
    /// [`DEFAULT_MAX_READ_LEN`]). Larger reads fail with `ResourceLimit`.
    pub fn with_max_read_len(mut self, max: u64) -> Self {
        self.max_read_len = max;
        self
    }

    /// Cap the handles open at once (default [`DEFAULT_MAX_HANDLES`]).
    /// An `open` beyond it fails with `ResourceLimit`; close a handle first.
    pub fn with_max_handles(mut self, max: usize) -> Self {
        self.max_handles = max;
        self
    }

    /// The confinement root, if this store is rooted.
    pub fn root(&self) -> Option<&std::path::Path> {
        self.root.as_deref()
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn root_listing() -> Value {
        Value::Map(btree! {
            "handles".into() => Reference::with_type("handles", "collection").to_value(),
            "results".into() => Reference::with_type("results", "collection").to_value(),
            "open".into() => Reference::with_type("meta/open", "action").to_value(),
            "stat".into() => Reference::with_type("meta/stat", "action").to_value(),
            "readdir".into() => Reference::with_type("meta/readdir", "action").to_value(),
            "mkdir".into() => Reference::with_type("meta/mkdir", "action").to_value(),
            "rmdir".into() => Reference::with_type("meta/rmdir", "action").to_value(),
            "unlink".into() => Reference::with_type("meta/unlink", "action").to_value(),
            "rename".into() => Reference::with_type("meta/rename", "action").to_value(),
            "meta".into() => Reference::with_type("meta", "meta").to_value(),
        })
    }
}

impl Default for FsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for FsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(Self::root_listing())));
        }
        let value = match &from[0] {
            "meta" => self.read_meta(&from.slice(1, from.len()))?,
            "handles" if from.len() == 1 => Some(self.handles_listing()),
            "handles" => {
                let (id, op) = parse_handle_operation(from).ok_or_else(|| {
                    Error::invalid_argument(format!("invalid handle path: {from}"))
                })?;
                Some(self.read_handle(id, op)?)
            }
            "results" if from.len() == 1 => Some(self.results_listing()),
            "results" => from[1]
                .parse::<u64>()
                .ok()
                .and_then(|id| self.results.get(&id))
                .and_then(|value| value.get(&from.slice(2, from.len())))
                .cloned(),
            _ => None,
        };
        Ok(value.map(Record::parsed))
    }
}

impl Writer for FsStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::permission_denied("cannot write to the fs root"));
        }
        let value = data.into_value(&NoCodec)?;
        match &to[0] {
            "meta" => {
                self.write_meta(&to.slice(1, to.len()), &value)?;
                Ok(to.clone())
            }
            "handles" => self.write_handle(to, &value),
            "results" => self.write_result(to, &value),
            "open" if to.len() == 1 => self.open(&value),
            action if to.len() == 1 => self.action(action, &value),
            _ => Err(Error::invalid_argument(format!("invalid fs path: {to}"))),
        }
    }
}

#[cfg(test)]
mod tests;
