//! Durable persistence for in-memory stores.
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

    /// Persist the root.
    fn save(&mut self, root: &Value) -> Result<(), Error>;
}

/// Whole-file JSON persistence.
///
/// `save` writes atomically: the new contents go to a sibling temp file
/// which is renamed over the target, so a crash mid-save never leaves a
/// truncated file.
pub struct JsonFileBacking {
    path: PathBuf,
}

impl JsonFileBacking {
    /// Persist to the given file path. The file need not exist yet; parent
    /// directories are created on first save.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
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
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(root)
            .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// An [`InMemoryStore`] persisted through a [`Backing`].
///
/// Reads are served from memory. Every successful write is saved through
/// the backing before returning, so on-disk state never lags memory.
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
}

impl<B: Backing> BackedStore<B> {
    /// Open a store, loading existing state from the backing.
    pub fn open(mut backing: B) -> Result<Self, Error> {
        let inner = match backing.load()? {
            Some(root) => InMemoryStore::with_data(root),
            None => InMemoryStore::new(),
        };
        Ok(Self { inner, backing })
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
        let result = self.inner.write(to, data)?;
        self.backing.save(self.inner.root())?;
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
