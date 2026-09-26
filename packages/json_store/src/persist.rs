//! Persistence with explicit acknowledgement and recovery boundaries.
//!
//! A [`Backing`] abstracts where a store's root `Value` lives at rest
//! (a file, a database row, browser storage, a remote blob), and
//! [`BackedStore`] pairs a [`MemoryStore`] with a backing: state is
//! loaded once at open and saved after every successful write.
//!
//! # On-disk format
//!
//! [`JsonFileBacking`] writes **StructFS Value JSON v1** — the
//! `["structfs-value",1,…]` envelope produced by
//! [`structfs_serde_store::ValueJsonCodec`]. Plain `serde_json` was used
//! before 0.5 and is lossy: it turns `Value::Bytes` into an array of
//! numbers, and NaN/±inf into `null`. The tagged form round-trips both.
//!
//! Files written by 0.4 and earlier are still readable: `load` tries the
//! tagged form first and falls back to plain JSON. Once such a file is
//! saved again it is in the new form, and older versions of this crate can
//! no longer read it — the format change is one-way.
//!
//! The fallback is ordered, not sniffed: a legacy plain-JSON document that
//! happens to be literally the array `["structfs-value", 1, <valid tagged
//! body>]` is read as the tagged form, i.e. as the value that body encodes
//! rather than as a three-element array.
//!
//! Both loads **and saves** are bounded by the backing's
//! [`Limits`] — [`Limits::default()`] (16 MiB, depth 64, 262144 nodes,
//! 65536 entries per collection) unless replaced with `with_limits`. A file
//! past the bounds fails to load, and a tree that has outgrown them fails to
//! save; raise the limits for stores expected to be larger. A save rejected
//! by the bounds fails before the file is touched, so the store stays
//! writable and unchanged.
//!
//! Depth has one bound the limits cannot raise: the tagged form carries at
//! most 84 levels of nesting (it spends up to three JSON levels per value
//! level, and the decoder's recursion ceiling is fixed). A legacy plain-JSON
//! file nested deeper than that still loads, with raised limits, but cannot
//! be saved again in the tagged form; the save fails cleanly.

use std::path::PathBuf;

use structfs_core_store::{
    Bytes, Codec, Error, Format, MemoryStore, Path, Reader, Record, Value, Writer,
};
use structfs_serde_store::{CodecProfile, Limits, ValueCodec};

/// Where a store's root `Value` is persisted.
pub trait Backing: Send + Sync {
    /// Load the persisted root, or `None` if nothing has been saved yet.
    fn load(&mut self) -> Result<Option<Value>, Error>;

    /// Save the root at this backing's documented acknowledgement level.
    /// An error may occur after replacement: it never implies disk rollback.
    fn save(&mut self, root: &Value) -> Result<(), Error>;

    /// Whether the most recent failed [`Backing::save`] may have changed the
    /// persisted state. [`BackedStore`] fences further writes until
    /// `recover` only when this is `true`.
    ///
    /// The default is the conservative `true`. A backing that can tell a
    /// failure *before* it touched storage (an encoding error, say) from one
    /// after should return `false` for the former, so such a failure leaves
    /// the store writable.
    fn last_failure_ambiguous(&self) -> bool {
        true
    }
}

/// Acknowledgement level for file backings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Durability {
    /// Writes reached the operating system; no power-loss guarantee.
    #[default]
    Buffered,
    /// Synchronize file contents and the parent directory before acknowledging.
    /// Requires an existing parent directory and directory-sync support.
    Synced,
}

/// A point inside a durable write, named so tests can inject a fault at
/// exactly one of them. Crate-private: the production path passes a
/// closure that ignores the stage and returns `Ok`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteStage {
    /// Before the payload bytes are written.
    Write,
    /// Before the file is synchronized.
    Sync,
    /// Before the temp file is renamed over the target.
    Rename,
    /// Before the parent directory is synchronized.
    DirectorySync,
}

pub(crate) fn parent(path: &std::path::Path) -> &std::path::Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."))
}
pub(crate) fn sync_parent(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(parent(path))?.sync_all()
}

/// Serialize one document in the on-disk form (StructFS Value JSON v1).
pub(crate) fn encode_document(value: &Value, limits: &Limits) -> Result<Vec<u8>, Error> {
    ValueCodec::new(CodecProfile::ValueJson)
        .with_limits(limits.clone())
        .encode(value, &Format::VALUE_JSON)
        .map(|bytes| bytes.to_vec())
}

/// Parse one on-disk document, accepting the current tagged form and the
/// plain JSON written before 0.5. Both errors are real failures; the tagged
/// one is reported because it names the format this crate writes.
pub(crate) fn decode_document(bytes: &[u8], limits: &Limits) -> Result<Value, Error> {
    let bytes = Bytes::copy_from_slice(bytes);
    let tagged = ValueCodec::new(CodecProfile::ValueJson).with_limits(limits.clone());
    match tagged.decode(&bytes, &Format::VALUE_JSON) {
        Ok(value) => Ok(value),
        Err(error) => ValueCodec::new(CodecProfile::Json)
            .with_limits(limits.clone())
            .decode(&bytes, &Format::JSON)
            .map_err(|_| error),
    }
}

/// Whole-file persistence of a `Value` in StructFS Value JSON v1.
///
/// `save` writes atomically: the new contents go to a sibling temp file
/// which is renamed over the target. Buffered mode promises atomic replacement
/// during normal operation, not power-loss durability. Synced mode synchronizes
/// the temp file before replacement and the parent directory afterwards (including
/// new files). A failure after replacement is ambiguous: reopen to reconcile.
/// Separate writers require external serialization; this is not a transaction
/// coordinator. Filesystems must support atomic rename and the requested syncs.
///
/// See the [module docs](self#on-disk-format) for the format and for what
/// happens to files written by earlier versions.
pub struct JsonFileBacking {
    path: PathBuf,
    durability: Durability,
    limits: Limits,
    /// Set once a save reaches the rename that replaces the target file; a
    /// failure before that point left the persisted state untouched.
    replacing: bool,
}

impl JsonFileBacking {
    /// Persist to the given file path. The file need not exist yet; parent
    /// directories are created on first save.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            durability: Durability::Buffered,
            limits: Limits::default(),
            replacing: false,
        }
    }

    /// Set the acknowledgement level. Defaults to [`Durability::Buffered`].
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// Set the bounds applied to every load and save. Defaults to
    /// [`Limits::default()`]; see the [module docs](self#on-disk-format).
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    fn save_with(
        &mut self,
        root: &Value,
        mut before: impl FnMut(WriteStage) -> std::io::Result<()>,
    ) -> Result<(), Error> {
        use std::io::Write;
        self.replacing = false;
        // Encode first: a value the limits reject fails before anything on
        // disk (even the parent directory) is created.
        let bytes = encode_document(root, &self.limits)?;
        if self.durability == Durability::Buffered {
            std::fs::create_dir_all(parent(&self.path))?;
        }
        let mut tmp = tempfile::NamedTempFile::new_in(parent(&self.path))?;
        before(WriteStage::Write)?;
        tmp.write_all(&bytes)?;
        if self.durability == Durability::Synced {
            before(WriteStage::Sync)?;
            tmp.as_file().sync_all()?;
        }
        before(WriteStage::Rename)?;
        self.replacing = true;
        tmp.persist(&self.path).map_err(|e| Error::Io(e.error))?;
        if self.durability == Durability::Synced {
            before(WriteStage::DirectorySync)?;
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
        decode_document(&bytes, &self.limits).map(Some)
    }

    fn save(&mut self, root: &Value) -> Result<(), Error> {
        self.save_with(root, |_| Ok(()))
    }

    /// Failures before the rename (encoding, temp-file creation, writing or
    /// syncing the temp file) leave the target file as it was.
    fn last_failure_ambiguous(&self) -> bool {
        self.replacing
    }
}

/// Open a snapshot as a store. A persisted `Null` root is an empty store:
/// under the store conventions writing `Null` at the root *is* deleting
/// everything, so that is what an empty store saves as.
fn store_from(root: Option<Value>) -> MemoryStore {
    match root {
        Some(root) if !root.is_null() => MemoryStore::with_root(root),
        _ => MemoryStore::new(),
    }
}

/// A [`MemoryStore`] persisted through a [`Backing`].
///
/// Reads see the last acknowledged value. Writes stage a candidate, save it,
/// then publish it in memory. A save error that may already have changed disk
/// ([`Backing::last_failure_ambiguous`]) fences subsequent writes until
/// `recover` reloads the backing. Failures before disk is touched (limits,
/// encoding, temp-file creation) leave the store writable and unchanged.
/// Recovery explicitly adopts the backing's readable state, which can include an
/// unacknowledged write. Do not blindly retry a non-idempotent operation.
///
/// Store conventions (reading a prefix returns its children, `Null` deletes a
/// subtree, a map write replaces one) are exactly `MemoryStore`'s — this type
/// adds durability, not semantics.
///
/// # Example
///
/// ```rust,no_run
/// use structfs_json_store::persist::{BackedStore, JsonFileBacking};
/// use structfs_core_store::{path, Reader, Writer, Record, Value};
///
/// let mut store = BackedStore::open(JsonFileBacking::new("config.json")).unwrap();
/// store.write(&path!("debug"), Record::parsed(Value::Bool(true))).unwrap();
/// // config.json now holds the Value JSON v1 encoding of {"debug": true}
/// ```
pub struct BackedStore<B: Backing> {
    inner: MemoryStore,
    backing: B,
    needs_recovery: bool,
}

impl<B: Backing> BackedStore<B> {
    /// Open a store, loading existing state from the backing.
    pub fn open(mut backing: B) -> Result<Self, Error> {
        let inner = store_from(backing.load()?);
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
        self.inner = store_from(self.backing.load()?);
        self.needs_recovery = false;
        Ok(())
    }

    /// Whether a save failed ambiguously and writes are fenced until
    /// [`BackedStore::recover`] succeeds.
    pub fn needs_recovery(&self) -> bool {
        self.needs_recovery
    }

    /// Borrow the in-memory root. `None` is an empty store, matching
    /// [`MemoryStore::root`].
    pub fn root(&self) -> Option<&Value> {
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

    fn read_children_page(
        &mut self,
        from: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Option<structfs_core_store::ChildPage>, Error> {
        self.inner.read_children_page(from, offset, limit)
    }
}

impl<B: Backing> Writer for BackedStore<B> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if self.needs_recovery {
            return Err(Error::conflict(
                "save failed; recover backing before writing",
            ));
        }
        let mut candidate = match self.inner.root() {
            Some(root) => MemoryStore::with_root(root.clone()),
            None => MemoryStore::new(),
        };
        let result = candidate.write(to, data)?;
        // Fenced for the duration of the save (so a panic inside it leaves the
        // store fenced), then unfenced again if the backing reports that the
        // failure happened before storage was touched.
        self.needs_recovery = true;
        if let Err(error) = self.backing.save(candidate.root().unwrap_or(&Value::Null)) {
            self.needs_recovery = self.backing.last_failure_ambiguous();
            return Err(error);
        }
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
        assert_eq!(store.root(), None);
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

    #[test]
    fn children_are_enumerated_through_the_backing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = BackedStore::open(JsonFileBacking::new(dir.path().join("s"))).unwrap();
        store
            .write(&path!("a/one"), Record::parsed(Value::from(1i64)))
            .unwrap();
        store
            .write(&path!("a/two"), Record::parsed(Value::from(2i64)))
            .unwrap();

        assert_eq!(
            store.read_children(&path!("a")).unwrap().unwrap(),
            vec!["one".to_string(), "two".to_string()]
        );
        let page = store
            .read_children_page(&path!("a"), 0, 1)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["one".to_string()]);
        assert_eq!(page.next, Some(1));
    }

    /// The reason the format changed: plain JSON cannot carry these.
    #[test]
    fn bytes_and_non_finite_floats_survive_a_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lossless.json");

        {
            let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
            store
                .write(
                    &path!("blob"),
                    Record::parsed(Value::Bytes(vec![0, 159, 146, 150])),
                )
                .unwrap();
            store
                .write(&path!("inf"), Record::parsed(Value::Float(f64::INFINITY)))
                .unwrap();
            store
                .write(
                    &path!("neg_inf"),
                    Record::parsed(Value::Float(f64::NEG_INFINITY)),
                )
                .unwrap();
            store
                .write(&path!("nan"), Record::parsed(Value::Float(f64::NAN)))
                .unwrap();
            store
                .write(&path!("big"), Record::parsed(Value::Unsigned(u64::MAX)))
                .unwrap();
        }

        let mut reopened = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        assert_eq!(
            reopened.read(&path!("blob")).unwrap().unwrap().as_value(),
            Some(&Value::Bytes(vec![0, 159, 146, 150]))
        );
        assert_eq!(
            reopened.read(&path!("inf")).unwrap().unwrap().as_value(),
            Some(&Value::Float(f64::INFINITY))
        );
        assert_eq!(
            reopened
                .read(&path!("neg_inf"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::Float(f64::NEG_INFINITY))
        );
        match reopened.read(&path!("nan")).unwrap().unwrap().as_value() {
            Some(Value::Float(f)) => assert!(f.is_nan()),
            other => panic!("expected a NaN float, got {other:?}"),
        }
        assert_eq!(
            reopened.read(&path!("big")).unwrap().unwrap().as_value(),
            Some(&Value::Unsigned(u64::MAX))
        );
    }

    /// Files written by 0.4 and earlier hold plain JSON; they still load.
    #[test]
    fn legacy_plain_json_files_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("legacy.json");
        std::fs::write(
            &file,
            b"{\n  \"users\": {\n    \"alice\": \"Alice\"\n  }\n}",
        )
        .unwrap();

        let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        assert_eq!(
            store
                .read(&path!("users/alice"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("Alice"))
        );

        // Saving migrates the file to the tagged form.
        store
            .write(&path!("users/bob"), Record::parsed(Value::from("Bob")))
            .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.starts_with("[\"structfs-value\",1,"),
            "expected the tagged form, got {text}"
        );
    }

    fn nested(depth: usize) -> Value {
        let mut value = Value::from(1i64);
        for _ in 0..depth {
            value = Value::Map([("k".to_string(), value)].into_iter().collect());
        }
        value
    }

    fn legacy_nested(depth: usize) -> String {
        let mut legacy = String::from("1");
        for _ in 0..depth {
            legacy = format!("{{\"k\":{legacy}}}");
        }
        legacy
    }

    /// A 0.4 file deeper than the default bounds loads once they are raised,
    /// and saves and reloads under the same bounds.
    #[test]
    fn limits_govern_loading_and_can_be_raised() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep.json");
        std::fs::write(&file, legacy_nested(80)).unwrap();

        assert!(BackedStore::open(JsonFileBacking::new(&file)).is_err());

        let raised =
            || JsonFileBacking::new(&file).with_limits(Limits::default().with_max_depth(200));
        let mut store = BackedStore::open(raised()).unwrap();
        assert_eq!(store.root(), Some(&nested(80)));
        store
            .write(&path!("other"), Record::parsed(Value::from(2i64)))
            .unwrap();
        let reopened = BackedStore::open(raised()).unwrap();
        assert_eq!(reopened.root(), store.root());
    }

    /// A 0.4 file deeper than the tagged form can carry still loads, but a
    /// save of it is refused cleanly rather than writing a file that could
    /// not be read back.
    #[test]
    fn legacy_file_deeper_than_the_tagged_ceiling_loads_but_cannot_be_resaved() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deeper.json");
        std::fs::write(&file, legacy_nested(100)).unwrap();
        let before = std::fs::read(&file).unwrap();

        let mut store = BackedStore::open(
            JsonFileBacking::new(&file).with_limits(Limits::default().with_max_depth(200)),
        )
        .unwrap();
        assert_eq!(store.root(), Some(&nested(100)));
        assert!(store
            .write(&path!("other"), Record::parsed(Value::from(2i64)))
            .is_err());
        assert!(!store.needs_recovery());
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    /// A save the bounds reject fails before touching disk: the store keeps
    /// its acknowledged state and stays writable.
    #[test]
    fn save_rejected_by_limits_leaves_store_writable_and_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bounded.json");
        let backing =
            || JsonFileBacking::new(&file).with_limits(Limits::default().with_max_depth(4));
        let mut store = BackedStore::open(backing()).unwrap();
        store
            .write(&path!("ok"), Record::parsed(Value::from(1i64)))
            .unwrap();
        let before = std::fs::read(&file).unwrap();

        assert!(store
            .write(&path!("deep"), Record::parsed(nested(10)))
            .is_err());
        assert!(!store.needs_recovery());
        assert!(store.read(&path!("deep")).unwrap().is_none());
        assert_eq!(std::fs::read(&file).unwrap(), before);

        store
            .write(&path!("after"), Record::parsed(Value::from(2i64)))
            .unwrap();
        let mut reopened = BackedStore::open(backing()).unwrap();
        assert!(reopened.read(&path!("after")).unwrap().is_some());
    }

    /// Deleting the root persists `null`; that reopens as an empty store,
    /// not as a store holding a Null value.
    #[test]
    fn persisted_null_root_reopens_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("null.json");
        {
            let mut store = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
            store
                .write(&path!("x"), Record::parsed(Value::from(1i64)))
                .unwrap();
            store
                .write(&path!(""), Record::parsed(Value::Null))
                .unwrap();
        }
        let mut reopened = BackedStore::open(JsonFileBacking::new(&file)).unwrap();
        assert_eq!(reopened.root(), None);
        assert!(reopened.read(&path!("")).unwrap().is_none());
        reopened.recover().unwrap();
        assert_eq!(reopened.root(), None);
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use structfs_core_store::path;

    #[test]
    fn failures_before_and_after_replacement_have_explicit_reopen_state() {
        for stage in [
            WriteStage::Write,
            WriteStage::Sync,
            WriteStage::Rename,
            WriteStage::DirectorySync,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut backing =
                JsonFileBacking::new(dir.path().join("state")).with_durability(Durability::Synced);
            backing.save(&Value::Integer(1)).unwrap();
            let mut order = Vec::new();
            assert!(backing
                .save_with(&Value::Integer(2), |point| {
                    order.push(point);
                    if point == stage {
                        Err(std::io::Error::other("injected"))
                    } else {
                        Ok(())
                    }
                })
                .is_err());
            assert_eq!(order.last(), Some(&stage));
            // Only a failure after the rename began can have changed the file.
            assert_eq!(
                backing.last_failure_ambiguous(),
                stage == WriteStage::DirectorySync
            );
            assert_eq!(
                backing.load().unwrap(),
                Some(Value::Integer(if stage == WriteStage::DirectorySync {
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
        assert_eq!(store.root(), Some(&Value::Integer(1)));
        assert!(store.needs_recovery());
        assert!(store
            .write(&path!(""), Record::parsed(Value::Integer(3)))
            .is_err());
        store.recover().unwrap();
        assert_eq!(store.root(), Some(&Value::Integer(2)));
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
