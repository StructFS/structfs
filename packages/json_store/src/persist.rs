//! Persistence with explicit acknowledgement and recovery boundaries.
//!
//! A [`Backing`] abstracts where a store's root `Value` lives at rest
//! (a file, a database row, browser storage, a remote blob), and
//! [`BackedStore`] pairs an [`InMemoryStore`] with a backing: state is
//! loaded once at open and saved after every successful write.

use std::path::PathBuf;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

use crate::in_memory::InMemoryStore;

/// Where a store's root `Value` is persisted.
pub trait Backing: Send + Sync {
    /// Load the persisted root, or `None` if nothing has been saved yet.
    fn load(&mut self) -> Result<Option<Value>, Error>;

    /// Save the root at this backing's documented acknowledgement level.
    /// An error may occur after replacement: it never implies disk rollback.
    fn save(&mut self, root: &Value) -> Result<(), Error>;
}

/// Acknowledgement level for file backings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Durability {
    /// Writes reached the operating system; no power-loss guarantee.
    #[default]
    Buffered,
    /// Synchronize file contents and the parent directory before acknowledging.
    /// Requires an existing parent directory and directory-sync support.
    Synced,
}

pub(crate) fn parent(path: &std::path::Path) -> &std::path::Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
}
pub(crate) fn sync_parent(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(parent(path))?.sync_all()
}

/// Whole-file JSON persistence.
///
/// `save` writes atomically: the new contents go to a sibling temp file
/// which is renamed over the target. Buffered mode promises atomic replacement
/// during normal operation, not power-loss durability. Synced mode synchronizes
/// the temp file before replacement and the parent directory afterwards (including
/// new files). A failure after replacement is ambiguous: reopen to reconcile.
/// Separate writers require external serialization; this is not a transaction
/// coordinator. Filesystems must support atomic rename and the requested syncs.
pub struct JsonFileBacking {
    path: PathBuf,
    durability: Durability,
}

impl JsonFileBacking {
    /// Persist to the given file path. The file need not exist yet; parent
    /// directories are created on first save.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            durability: Durability::Buffered,
        }
    }
}

impl JsonFileBacking {
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }
    fn save_with(
        &mut self,
        root: &Value,
        mut before: impl FnMut(&str) -> std::io::Result<()>,
    ) -> Result<(), Error> {
        use std::io::Write;
        if self.durability == Durability::Buffered {
            std::fs::create_dir_all(parent(&self.path))?;
        }
        let bytes = serde_json::to_vec_pretty(root)
            .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent(&self.path))?;
        before("write")?;
        tmp.write_all(&bytes)?;
        if self.durability == Durability::Synced {
            before("sync")?;
            tmp.as_file().sync_all()?;
        }
        before("rename")?;
        tmp.persist(&self.path).map_err(|e| Error::Io(e.error))?;
        if self.durability == Durability::Synced {
            before("directory_sync")?;
            sync_parent(&self.path)?;
        }
        Ok(())
    }
}

impl Backing for JsonFileBacking {
    fn load(&mut self) -> Result<Option<Value>, Error> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::Io(e)),
        };
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::decode(structfs_core_store::Format::JSON, e.to_string()))?;
        Ok(Some(value))
    }

    fn save(&mut self, root: &Value) -> Result<(), Error> {
        self.save_with(root, |_| Ok(()))
    }
}

/// An [`InMemoryStore`] persisted through a [`Backing`].
///
/// Reads see the last acknowledged value. Writes stage a candidate, save it,
/// then publish it in memory. Any save error fences subsequent writes until
/// `recover` reloads the backing: a failed save may already have changed disk.
/// Recovery explicitly adopts the backing's readable state, which can include an
/// unacknowledged write. Do not blindly retry a non-idempotent operation.
///
/// # Example
///
/// ```rust,no_run
/// use structfs_json_store::persist::{BackedStore, JsonFileBacking};
/// use structfs_core_store::{path, Reader, Writer, Record, Value};
///
/// let mut store = BackedStore::open(JsonFileBacking::new("config.json")).unwrap();
/// store.write(&path!("debug"), Record::parsed(Value::Bool(true))).unwrap();
/// // config.json now contains {"debug": true}
/// ```
pub struct BackedStore<B: Backing> {
    inner: InMemoryStore,
    backing: B,
    needs_recovery: bool,
}

impl<B: Backing> BackedStore<B> {
    /// Open a store, loading existing state from the backing.
    pub fn open(mut backing: B) -> Result<Self, Error> {
        let inner = match backing.load()? {
            Some(root) => InMemoryStore::with_data(root),
            None => InMemoryStore::new(),
        };
        Ok(Self {
            inner,
            backing,
            needs_recovery: false,
        })
    }

    /// Reconcile an ambiguous save by loading the backing. A failed recovery
    /// retains the old readable value and keeps writes fenced.
    pub fn recover(&mut self) -> Result<(), Error> {
        self.needs_recovery = true;
        let root = self.backing.load()?.unwrap_or(Value::Null);
        self.inner = InMemoryStore::with_data(root);
        self.needs_recovery = false;
        Ok(())
    }
    pub fn needs_recovery(&self) -> bool {
        self.needs_recovery
    }

    /// Access the in-memory state.
    pub fn root(&self) -> &Value {
        self.inner.root()
    }
}

impl<B: Backing> Reader for BackedStore<B> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.inner.read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        self.inner.read_children(from)
    }
}

impl<B: Backing> Writer for BackedStore<B> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if self.needs_recovery {
            return Err(Error::conflict(
                "save failed; recover backing before writing",
            ));
        }
        let mut candidate = InMemoryStore::with_data(self.inner.root().clone());
        let result = candidate.write(to, data)?;
        self.needs_recovery = true;
        self.backing.save(candidate.root())?;
        self.inner = candidate;
        self.needs_recovery = false;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.json");

        {
            let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
            store
                .write(&path!("users/alice"), Record::parsed(Value::from("Alice")))
                .unwrap();
        }

        let mut reopened = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        let record = reopened.read(&path!("users/alice")).unwrap().unwrap();
        assert_eq!(record.as_value(), Some(&Value::from("Alice")));
    }

    #[test]
    fn missing_file_opens_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nonexistent.json");

        let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        assert!(store.read(&path!("anything")).unwrap().is_none());
    }

    #[test]
    fn creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested/deeper/store.json");

        let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        store
            .write(&path!("key"), Record::parsed(Value::from(1i64)))
            .unwrap();
        assert!(file.exists());
    }

    #[test]
    fn null_delete_is_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.json");

        {
            let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
            store
                .write(&path!("temp"), Record::parsed(Value::from("x")))
                .unwrap();
            store
                .write(&path!("temp"), Record::parsed(Value::Null))
                .unwrap();
        }

        let mut reopened = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        assert!(reopened.read(&path!("temp")).unwrap().is_none());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("store.json");
        std::fs::write(&file, b"not json {{{").unwrap();

        assert!(BackedStore::open(JsonFileBacking::new(&file)).is_err());
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use structfs_core_store::path;
    #[test]
    fn failures_before_and_after_replacement_have_explicit_reopen_state() {
        for stage in ["write", "sync", "rename", "directory_sync"] {
            let dir = tempfile::tempdir().unwrap();
            let mut backing =
                JsonFileBacking::new(dir.path().join("state")).with_durability(Durability::Synced);
            backing.save(&Value::Integer(1)).unwrap();
            let mut order = Vec::new();
            assert!(backing
                .save_with(&Value::Integer(2), |point| {
                    order.push(point.to_owned());
                    if point == stage {
                        Err(std::io::Error::other("injected"))
                    } else {
                        Ok(())
                    }
                })
                .is_err());
            assert_eq!(order.last().unwrap(), stage);
            assert_eq!(
                backing.load().unwrap(),
                Some(Value::Integer(if stage == "directory_sync" {
                    2
                } else {
                    1
                }))
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn failed_save_keeps_acknowledged_memory_and_fences_retry_until_recovery() {
        struct Ambiguous {
            root: Value,
        }
        impl Backing for Ambiguous {
            fn load(&mut self) -> Result<Option<Value>, Error> {
                Ok(Some(self.root.clone()))
            }
            fn save(&mut self, root: &Value) -> Result<(), Error> {
                self.root = root.clone();
                Err(Error::Io(std::io::Error::other("after replacement")))
            }
        }
        let mut store = BackedStore::open(Ambiguous {
            root: Value::Integer(1),
        })
        .unwrap();
        assert!(store
            .write(&path!(""), Record::parsed(Value::Integer(2)))
            .is_err());
        assert_eq!(store.root(), &Value::Integer(1));
        assert!(store.needs_recovery());
        assert!(store
            .write(&path!(""), Record::parsed(Value::Integer(3)))
            .is_err());
        store.recover().unwrap();
        assert_eq!(store.root(), &Value::Integer(2));
        assert!(!store.needs_recovery());
    }
    #[test]
    fn synchronized_creation_reopens_and_requires_an_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let mut backing =
            JsonFileBacking::new(dir.path().join("new")).with_durability(Durability::Synced);
        backing.save(&Value::Null).unwrap();
        assert_eq!(backing.load().unwrap(), Some(Value::Null));
        let mut missing = JsonFileBacking::new(dir.path().join("missing/new"))
            .with_durability(Durability::Synced);
        assert!(missing.save(&Value::Null).is_err());
    }
}
