//! OverlayStore: Route reads/writes to different stores based on path prefixes.
//!
//! This is a simpler version than the legacy OverlayStore, working with Records
//! instead of generic serde types and avoiding complex type erasure.

use crate::{Error, Path, PathError, Reader, Record, Writer};

/// A boxed store that is Send + Sync.
pub type StoreBox = Box<dyn Store + Send + Sync>;

/// Combined read/write trait for stores.
pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}

/// Wraps a Reader to reject all writes.
pub struct OnlyReadable<R> {
    inner: R,
}

impl<R> OnlyReadable<R> {
    /// Create a new read-only wrapper.
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Get a reference to the inner reader.
    pub fn inner(&self) -> &R {
        &self.inner
    }
}

impl<R: Reader> Reader for OnlyReadable<R> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.inner.read(from)
    }
}

impl<R: Reader + Send + Sync> Writer for OnlyReadable<R> {
    fn write(&mut self, to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::Path(PathError::InvalidPath {
            message: format!("Path '{}' is read-only", to),
        }))
    }
}

/// Wraps a Writer to reject all reads.
pub struct OnlyWritable<W> {
    inner: W,
}

impl<W> OnlyWritable<W> {
    /// Create a new write-only wrapper.
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Get a reference to the inner writer.
    pub fn inner(&self) -> &W {
        &self.inner
    }
}

impl<W: Writer + Send + Sync> Reader for OnlyWritable<W> {
    fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
        Ok(None)
    }
}

impl<W: Writer> Writer for OnlyWritable<W> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.inner.write(to, data)
    }
}

/// Views into a sub-path of a store.
pub struct SubStoreView<S> {
    inner: S,
    prefix: Path,
}

impl<S> SubStoreView<S> {
    /// Create a new sub-store view.
    pub fn new(inner: S, prefix: Path) -> Self {
        Self { inner, prefix }
    }
}

impl<S: Reader> Reader for SubStoreView<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.inner.read(&self.prefix.join(from))
    }
}

impl<S: Writer> Writer for SubStoreView<S> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.inner.write(&self.prefix.join(to), data)
    }
}

/// Route reads and writes to different stores based on path prefixes.
///
/// Later layers take priority over earlier ones (last-added wins for overlapping paths).
///
/// # Example
///
/// ```rust
/// use structfs_core_store::{Reader, Writer, Record, Value, path};
/// use structfs_core_store::overlay_store::OverlayStore;
///
/// // Create stores (in practice, these would be real store implementations)
/// // ... setup ...
///
/// let mut overlay = OverlayStore::new();
/// // overlay.add_layer(path!("users"), user_store);
/// // overlay.add_layer(path!("config"), config_store);
/// ```
#[derive(Default)]
pub struct OverlayStore {
    routes: Vec<(Path, StoreBox)>,
}

impl OverlayStore {
    /// Create a new empty overlay store.
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    /// Add a store layer at the given path prefix.
    ///
    /// Later layers take priority for overlapping paths.
    pub fn add_layer<S: Store + Send + Sync + 'static>(&mut self, mount_root: Path, store: S) {
        self.routes.push((mount_root, Box::new(store)));
    }

    /// Add a read-only layer.
    pub fn add_read_only_layer<R: Reader + Send + Sync + 'static>(
        &mut self,
        mount_root: Path,
        reader: R,
    ) {
        self.add_layer(mount_root, OnlyReadable::new(reader));
    }

    /// Add a write-only layer.
    pub fn add_write_only_layer<W: Writer + Send + Sync + 'static>(
        &mut self,
        mount_root: Path,
        writer: W,
    ) {
        self.add_layer(mount_root, OnlyWritable::new(writer));
    }

    /// Match a path to a store, returning the store and the path suffix.
    fn match_store(&mut self, path: &Path) -> Option<(&mut StoreBox, Path)> {
        // Iterate in reverse to get last-added first (priority)
        for (prefix, store) in self.routes.iter_mut().rev() {
            if path.has_prefix(prefix) {
                let suffix = path.strip_prefix(prefix).unwrap();
                return Some((store, suffix));
            }
        }
        None
    }

    /// Get the number of layers.
    pub fn layer_count(&self) -> usize {
        self.routes.len()
    }
}

impl Reader for OverlayStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match self.match_store(from) {
            Some((store, suffix)) => store.read(&suffix),
            None => Err(Error::Other {
                message: format!("No route found for path: {}", from),
            }),
        }
    }
}

impl Writer for OverlayStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        match self.match_store(to) {
            Some((store, suffix)) => {
                // Write to the store with the suffix path
                let result = store.write(&suffix, data)?;
                // If the result path is relative to the suffix, we need to
                // reconstruct the full path by joining with the original prefix
                // Find the matching prefix again (this is a bit inefficient but correct)
                for (prefix, _) in self.routes.iter().rev() {
                    if to.has_prefix(prefix) {
                        return Ok(prefix.join(&result));
                    }
                }
                // Shouldn't happen, but return the result as-is
                Ok(result)
            }
            None => Err(Error::Other {
                message: format!("No route found for path: {}", to),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, Value};
    use std::collections::HashMap;

    // Simple in-memory store for testing
    struct TestStore {
        data: HashMap<Path, Record>,
    }

    impl TestStore {
        fn new(_name: &str) -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Reader for TestStore {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl Writer for TestStore {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[test]
    fn basic_routing() {
        let mut overlay = OverlayStore::new();

        let mut users_store = TestStore::new("users");
        users_store
            .write(&path!("alice"), Record::parsed(Value::from("Alice")))
            .unwrap();

        let mut config_store = TestStore::new("config");
        config_store
            .write(&path!("theme"), Record::parsed(Value::from("dark")))
            .unwrap();

        overlay.add_layer(path!("users"), users_store);
        overlay.add_layer(path!("config"), config_store);

        // Read from users
        let record = overlay.read(&path!("users/alice")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("Alice"));

        // Read from config
        let record = overlay.read(&path!("config/theme")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("dark"));
    }

    #[test]
    fn layer_priority() {
        let mut overlay = OverlayStore::new();

        let mut store1 = TestStore::new("first");
        store1
            .write(&path!("key"), Record::parsed(Value::from("first")))
            .unwrap();

        let mut store2 = TestStore::new("second");
        store2
            .write(&path!("key"), Record::parsed(Value::from("second")))
            .unwrap();

        // Add store1 first, then store2 at the same path - store2 should win
        overlay.add_layer(path!("data"), store1);
        overlay.add_layer(path!("data"), store2);

        let record = overlay.read(&path!("data/key")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("second"));
    }

    #[test]
    fn root_mount() {
        let mut overlay = OverlayStore::new();

        let mut store = TestStore::new("root");
        store
            .write(&path!("test"), Record::parsed(Value::from("value")))
            .unwrap();

        overlay.add_layer(path!(""), store);

        let record = overlay.read(&path!("test")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn no_route_error() {
        let mut overlay = OverlayStore::new();

        let result = overlay.read(&path!("nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn write_through_overlay() {
        let mut overlay = OverlayStore::new();
        overlay.add_layer(path!("data"), TestStore::new("data"));

        let result = overlay.write(&path!("data/key"), Record::parsed(Value::from("value")));
        assert!(result.is_ok());

        let record = overlay.read(&path!("data/key")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn only_readable_rejects_writes() {
        let mut overlay = OverlayStore::new();
        overlay.add_read_only_layer(path!("readonly"), TestStore::new("readonly"));

        let result = overlay.write(&path!("readonly/key"), Record::parsed(Value::from("value")));
        assert!(result.is_err());
    }

    #[test]
    fn only_writable_returns_none_on_read() {
        let mut overlay = OverlayStore::new();
        overlay.add_write_only_layer(path!("writeonly"), TestStore::new("writeonly"));

        let result = overlay.read(&path!("writeonly/key")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn only_readable_inner() {
        let store = TestStore::new("test");
        let readable = OnlyReadable::new(store);
        let _inner = readable.inner();
        // Just verify it compiles and can be called
    }

    #[test]
    fn only_writable_inner() {
        let store = TestStore::new("test");
        let writable = OnlyWritable::new(store);
        let _inner = writable.inner();
        // Just verify it compiles and can be called
    }

    #[test]
    fn sub_store_view_read() {
        let mut store = TestStore::new("test");
        store
            .write(&path!("prefix/key"), Record::parsed(Value::from("value")))
            .unwrap();

        let mut view = SubStoreView::new(store, path!("prefix"));

        // Reading "key" should read "prefix/key" from the inner store
        let result = view.read(&path!("key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn sub_store_view_write() {
        let store = TestStore::new("test");
        let mut view = SubStoreView::new(store, path!("prefix"));

        // Writing to "key" should write to "prefix/key" in the inner store
        view.write(&path!("key"), Record::parsed(Value::from("data")))
            .unwrap();

        let result = view.read(&path!("key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("data"));
    }

    #[test]
    fn layer_count() {
        let mut overlay = OverlayStore::new();
        assert_eq!(overlay.layer_count(), 0);

        overlay.add_layer(path!("a"), TestStore::new("a"));
        assert_eq!(overlay.layer_count(), 1);

        overlay.add_layer(path!("b"), TestStore::new("b"));
        assert_eq!(overlay.layer_count(), 2);
    }

    #[test]
    fn overlay_store_default() {
        let overlay = OverlayStore::default();
        assert_eq!(overlay.layer_count(), 0);
    }

    #[test]
    fn write_no_route_error() {
        let mut overlay = OverlayStore::new();
        let result = overlay.write(&path!("nonexistent"), Record::parsed(Value::Null));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No route"));
    }

    #[test]
    fn only_writable_write() {
        let store = TestStore::new("test");
        let mut writable = OnlyWritable::new(store);

        // Should be able to write
        let result = writable.write(&path!("key"), Record::parsed(Value::from("data")));
        assert!(result.is_ok());
    }

    #[test]
    fn only_readable_read() {
        let mut store = TestStore::new("test");
        store
            .write(&path!("key"), Record::parsed(Value::from("data")))
            .unwrap();

        let mut readable = OnlyReadable::new(store);

        // Should be able to read
        let result = readable.read(&path!("key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("data"));
    }

    #[test]
    fn only_readable_error_message() {
        let store = TestStore::new("test");
        let mut readable = OnlyReadable::new(store);

        let result = readable.write(&path!("test/path"), Record::parsed(Value::Null));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("read-only"));
        assert!(err.to_string().contains("test/path"));
    }

    #[test]
    fn write_returns_full_path() {
        let mut overlay = OverlayStore::new();
        overlay.add_layer(path!("prefix"), TestStore::new("test"));

        // Write returns the full path including the prefix
        let result = overlay.write(&path!("prefix/key"), Record::parsed(Value::from("data")));
        assert_eq!(result.unwrap(), path!("prefix/key"));
    }

    #[test]
    fn nested_prefix() {
        let mut overlay = OverlayStore::new();

        let mut store = TestStore::new("nested");
        store
            .write(&path!("deep/key"), Record::parsed(Value::from("value")))
            .unwrap();

        overlay.add_layer(path!("a/b"), store);

        let result = overlay.read(&path!("a/b/deep/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }
}
