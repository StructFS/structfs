//! OverlayStore: Route reads/writes to different stores based on path prefixes.
//!
//! This implementation uses a prefix trie for efficient routing with fallthrough
//! semantics - the deepest matching prefix handles the request.

use crate::path_trie::PathTrie;
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
/// Uses a prefix trie internally for efficient routing. When a path is accessed,
/// the store walks the trie to find the deepest mounted store that matches the
/// path prefix, then delegates to that store with the remaining suffix.
///
/// # Example
///
/// ```rust
/// use structfs_core_store::{Reader, Writer, Record, Value, path};
/// use structfs_core_store::overlay_store::OverlayStore;
///
/// // Create an overlay
/// let mut overlay = OverlayStore::new();
///
/// // Mount stores at different paths
/// // overlay.mount(path!("users"), user_store);
/// // overlay.mount(path!("config"), config_store);
///
/// // Reads to /users/alice go to user_store with path "alice"
/// // Reads to /config/theme go to config_store with path "theme"
/// ```
#[derive(Default)]
pub struct OverlayStore {
    trie: PathTrie<StoreBox>,
}

impl OverlayStore {
    /// Create a new empty overlay store.
    pub fn new() -> Self {
        Self {
            trie: PathTrie::new(),
        }
    }

    /// Mount a store at the given path prefix.
    ///
    /// Returns the previous store at that exact path if any.
    pub fn mount<S: Store + Send + Sync + 'static>(
        &mut self,
        path: Path,
        store: S,
    ) -> Option<StoreBox> {
        self.trie.insert(&path, Box::new(store))
    }

    /// Mount a boxed store at the given path prefix.
    ///
    /// Returns the previous store at that exact path if any.
    pub fn mount_boxed(&mut self, path: Path, store: StoreBox) -> Option<StoreBox> {
        self.trie.insert(&path, store)
    }

    /// Unmount store at exact path, keeping any nested mounts.
    ///
    /// Returns the removed store if found.
    pub fn unmount(&mut self, path: &Path) -> Option<StoreBox> {
        self.trie.remove(path)
    }

    /// Unmount entire subtree at path, returning it as a new OverlayStore.
    pub fn unmount_subtree(&mut self, path: &Path) -> Option<OverlayStore> {
        self.trie
            .remove_subtree(path)
            .map(|trie| OverlayStore { trie })
    }

    /// Check if any store would handle this path (fallthrough).
    pub fn has_route(&self, path: &Path) -> bool {
        self.trie.find_ancestor(path).is_some()
    }

    /// Number of mounted stores.
    pub fn store_count(&self) -> usize {
        self.trie.len()
    }

    /// True if no stores mounted.
    pub fn is_empty(&self) -> bool {
        self.trie.is_empty()
    }

    /// Iterate over all mount points.
    pub fn mounts(&self) -> impl Iterator<Item = (Path, &StoreBox)> {
        self.trie.iter()
    }

    // === Deprecated methods for backwards compatibility ===

    /// Add a store layer at the given path prefix.
    ///
    /// Deprecated: use `mount()` instead.
    #[deprecated(note = "use mount() instead")]
    pub fn add_layer<S: Store + Send + Sync + 'static>(&mut self, mount_root: Path, store: S) {
        self.mount(mount_root, store);
    }

    /// Add a read-only layer.
    pub fn add_read_only_layer<R: Reader + Send + Sync + 'static>(
        &mut self,
        mount_root: Path,
        reader: R,
    ) {
        self.mount(mount_root, OnlyReadable::new(reader));
    }

    /// Add a write-only layer.
    pub fn add_write_only_layer<W: Writer + Send + Sync + 'static>(
        &mut self,
        mount_root: Path,
        writer: W,
    ) {
        self.mount(mount_root, OnlyWritable::new(writer));
    }

    /// Get the number of layers.
    ///
    /// Deprecated: use `store_count()` instead.
    #[deprecated(note = "use store_count() instead")]
    pub fn layer_count(&self) -> usize {
        self.store_count()
    }

    /// Remove a layer by its exact prefix path.
    ///
    /// Deprecated: use `unmount()` instead.
    #[deprecated(note = "use unmount() instead")]
    pub fn remove_layer(&mut self, prefix: &Path) -> Option<StoreBox> {
        self.unmount(prefix)
    }
}

impl Reader for OverlayStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match self.trie.find_ancestor_mut(from) {
            Some((store, suffix)) => store.read(&suffix),
            None => Err(Error::NoRoute { path: from.clone() }),
        }
    }
}

impl Writer for OverlayStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        match self.trie.find_ancestor_mut(to) {
            Some((store, suffix)) => {
                let result_suffix = store.write(&suffix, data)?;
                // Reconstruct full path: prefix + result
                let prefix_len = to.len() - suffix.len();
                let prefix = Path {
                    components: to.components[..prefix_len].to_vec(),
                };
                Ok(prefix.join(&result_suffix))
            }
            None => Err(Error::NoRoute { path: to.clone() }),
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

        overlay.mount(path!("users"), users_store);
        overlay.mount(path!("config"), config_store);

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
    fn mount_replaces_existing() {
        let mut overlay = OverlayStore::new();

        let mut store1 = TestStore::new("first");
        store1
            .write(&path!("key"), Record::parsed(Value::from("first")))
            .unwrap();

        let mut store2 = TestStore::new("second");
        store2
            .write(&path!("key"), Record::parsed(Value::from("second")))
            .unwrap();

        // Mount store1, then replace with store2
        let old = overlay.mount(path!("data"), store1);
        assert!(old.is_none());

        let old = overlay.mount(path!("data"), store2);
        assert!(old.is_some());

        // store2 should be active
        let record = overlay.read(&path!("data/key")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("second"));

        // Only one store mounted
        assert_eq!(overlay.store_count(), 1);
    }

    #[test]
    fn root_mount() {
        let mut overlay = OverlayStore::new();

        let mut store = TestStore::new("root");
        store
            .write(&path!("test"), Record::parsed(Value::from("value")))
            .unwrap();

        overlay.mount(path!(""), store);

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
        overlay.mount(path!("data"), TestStore::new("data"));

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
    fn store_count() {
        let mut overlay = OverlayStore::new();
        assert_eq!(overlay.store_count(), 0);

        overlay.mount(path!("a"), TestStore::new("a"));
        assert_eq!(overlay.store_count(), 1);

        overlay.mount(path!("b"), TestStore::new("b"));
        assert_eq!(overlay.store_count(), 2);
    }

    #[test]
    fn overlay_store_default() {
        let overlay = OverlayStore::default();
        assert_eq!(overlay.store_count(), 0);
    }

    #[test]
    fn write_no_route_error() {
        let mut overlay = OverlayStore::new();
        let result = overlay.write(&path!("nonexistent"), Record::parsed(Value::Null));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no route"));
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
        overlay.mount(path!("prefix"), TestStore::new("test"));

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

        overlay.mount(path!("a/b"), store);

        let result = overlay.read(&path!("a/b/deep/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn unmount_existing() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("a"), TestStore::new("a"));
        overlay.mount(path!("b"), TestStore::new("b"));
        assert_eq!(overlay.store_count(), 2);

        // Unmount "a"
        let removed = overlay.unmount(&path!("a"));
        assert!(removed.is_some());
        assert_eq!(overlay.store_count(), 1);

        // Reading from "a" should now fail
        let result = overlay.read(&path!("a/key"));
        assert!(result.is_err());

        // "b" should still work
        overlay
            .write(&path!("b/key"), Record::parsed(Value::from("test")))
            .unwrap();
        let result = overlay.read(&path!("b/key"));
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn unmount_nonexistent() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("a"), TestStore::new("a"));

        // Try to unmount non-existent
        let removed = overlay.unmount(&path!("nonexistent"));
        assert!(removed.is_none());
        assert_eq!(overlay.store_count(), 1);
    }

    #[test]
    fn unmount_keeps_children() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));
        overlay.mount(path!("data/nested"), TestStore::new("nested"));
        assert_eq!(overlay.store_count(), 2);

        // Write to nested
        overlay
            .write(
                &path!("data/nested/key"),
                Record::parsed(Value::from("nested_value")),
            )
            .unwrap();

        // Unmount data (not nested)
        overlay.unmount(&path!("data"));
        assert_eq!(overlay.store_count(), 1);

        // nested should still work
        let result = overlay.read(&path!("data/nested/key")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn unmount_subtree_removes_all() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));
        overlay.mount(path!("data/cache"), TestStore::new("cache"));
        assert_eq!(overlay.store_count(), 2);

        let subtree = overlay.unmount_subtree(&path!("data")).unwrap();

        assert_eq!(subtree.store_count(), 2);
        assert!(overlay.is_empty());
    }

    #[test]
    fn deeper_mount_wins() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));
        overlay.mount(path!("data/special"), TestStore::new("special"));

        // Write to data/special/key goes to the special store
        overlay
            .write(
                &path!("data/special/key"),
                Record::parsed(Value::from("special")),
            )
            .unwrap();

        // Write to data/other/key goes to the data store
        overlay
            .write(
                &path!("data/other/key"),
                Record::parsed(Value::from("other")),
            )
            .unwrap();

        // Unmount special
        overlay.unmount(&path!("data/special"));

        // data should still work
        let result = overlay.read(&path!("data/other/key")).unwrap();
        assert!(result.is_some());

        // But data/special/key is now routed to data store (which doesn't have it)
        let result = overlay.read(&path!("data/special/key")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn has_route() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));

        assert!(overlay.has_route(&path!("data")));
        assert!(overlay.has_route(&path!("data/anything")));
        assert!(!overlay.has_route(&path!("other")));
    }

    #[test]
    fn is_empty() {
        let mut overlay = OverlayStore::new();
        assert!(overlay.is_empty());

        overlay.mount(path!("a"), TestStore::new("a"));
        assert!(!overlay.is_empty());

        overlay.unmount(&path!("a"));
        assert!(overlay.is_empty());
    }

    #[test]
    fn mounts_iteration() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("a"), TestStore::new("a"));
        overlay.mount(path!("b"), TestStore::new("b"));
        overlay.mount(path!("a/c"), TestStore::new("c"));

        let mounts: Vec<_> = overlay.mounts().map(|(p, _)| p).collect();
        assert_eq!(mounts.len(), 3);
    }

    #[test]
    fn mount_boxed() {
        let mut overlay = OverlayStore::new();
        let store: StoreBox = Box::new(TestStore::new("boxed"));
        overlay.mount_boxed(path!("boxed"), store);

        overlay
            .write(&path!("boxed/key"), Record::parsed(Value::from("value")))
            .unwrap();
        let result = overlay.read(&path!("boxed/key")).unwrap();
        assert!(result.is_some());
    }

    // Tests for deprecated methods to ensure backwards compatibility
    #[test]
    #[allow(deprecated)]
    fn deprecated_add_layer() {
        let mut overlay = OverlayStore::new();
        overlay.add_layer(path!("data"), TestStore::new("data"));

        overlay
            .write(&path!("data/key"), Record::parsed(Value::from("value")))
            .unwrap();
        let result = overlay.read(&path!("data/key")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    #[allow(deprecated)]
    fn deprecated_layer_count() {
        let mut overlay = OverlayStore::new();
        assert_eq!(overlay.layer_count(), 0);

        overlay.mount(path!("a"), TestStore::new("a"));
        assert_eq!(overlay.layer_count(), 1);
    }

    #[test]
    #[allow(deprecated)]
    fn deprecated_remove_layer() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("a"), TestStore::new("a"));

        let removed = overlay.remove_layer(&path!("a"));
        assert!(removed.is_some());
        assert!(overlay.is_empty());
    }

    #[test]
    fn fallthrough_routing() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));

        // Write via fallthrough (path goes deep but store is at data)
        overlay
            .write(
                &path!("data/deep/nested/key"),
                Record::parsed(Value::from("value")),
            )
            .unwrap();

        // Read via fallthrough
        let result = overlay.read(&path!("data/deep/nested/key")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn empty_path_mounts_at_root() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!(""), TestStore::new("root"));

        // Everything routes to root store
        overlay
            .write(&path!("any/path"), Record::parsed(Value::from("v")))
            .unwrap();
        let result = overlay.read(&path!("any/path")).unwrap();
        assert!(result.is_some());
    }
}
