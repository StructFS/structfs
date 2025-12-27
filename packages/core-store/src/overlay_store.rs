//! OverlayStore: Route reads/writes to different stores based on path prefixes.
//!
//! This implementation uses a prefix trie for efficient routing with fallthrough
//! semantics - the deepest matching prefix handles the request.
//!
//! Supports both direct store mounts and redirects (path aliases) with cycle detection.

use std::collections::HashSet;

use crate::path_trie::PathTrie;
use crate::{Error, Path, PathError, Reader, Record, Writer};

/// A boxed store that is Send + Sync.
pub type StoreBox = Box<dyn Store + Send + Sync>;

/// Combined read/write trait for stores.
pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}

/// Access control for redirects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    /// Allow reads through this redirect.
    ReadOnly,
    /// Allow writes through this redirect.
    WriteOnly,
    /// Allow both reads and writes.
    ReadWrite,
}

/// What a route points to.
pub enum RouteTarget {
    /// Direct store mount.
    Store(StoreBox),
    /// Redirect to another path in the overlay.
    Redirect {
        /// Target path to redirect to.
        target: Path,
        /// Access mode for this redirect.
        mode: RedirectMode,
        /// Which mount created this redirect (for cascade unmount).
        source_mount: Option<String>,
    },
}

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
/// Supports redirects (path aliases) that forward requests to other paths,
/// with cycle detection to prevent infinite loops.
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
pub struct OverlayStore {
    trie: PathTrie<RouteTarget>,
}

impl Default for OverlayStore {
    fn default() -> Self {
        Self::new()
    }
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
        match self.trie.insert(&path, RouteTarget::Store(Box::new(store))) {
            Some(RouteTarget::Store(s)) => Some(s),
            _ => None,
        }
    }

    /// Mount a boxed store at the given path prefix.
    ///
    /// Returns the previous store at that exact path if any.
    pub fn mount_boxed(&mut self, path: Path, store: StoreBox) -> Option<StoreBox> {
        match self.trie.insert(&path, RouteTarget::Store(store)) {
            Some(RouteTarget::Store(s)) => Some(s),
            _ => None,
        }
    }

    /// Unmount store at exact path, keeping any nested mounts.
    ///
    /// Returns the removed store if found.
    pub fn unmount(&mut self, path: &Path) -> Option<StoreBox> {
        match self.trie.remove(path) {
            Some(RouteTarget::Store(s)) => Some(s),
            _ => None,
        }
    }

    /// Unmount entire subtree at path, returning it as a new OverlayStore.
    pub fn unmount_subtree(&mut self, path: &Path) -> Option<OverlayStore> {
        self.trie
            .remove_subtree(path)
            .map(|trie| OverlayStore { trie })
    }

    /// Add a redirect from one path to another.
    ///
    /// When a path under `from` is accessed, it will be redirected to
    /// the corresponding path under `to`.
    pub fn add_redirect(
        &mut self,
        from: Path,
        to: Path,
        mode: RedirectMode,
        source_mount: Option<String>,
    ) {
        self.trie.insert(
            &from,
            RouteTarget::Redirect {
                target: to,
                mode,
                source_mount,
            },
        );
    }

    /// Remove all redirects created by a specific mount.
    pub fn remove_redirects_for_mount(&mut self, mount_name: &str) {
        // Collect paths to remove (can't mutate while iterating)
        let to_remove: Vec<Path> = self
            .trie
            .iter()
            .filter_map(|(path, target)| match target {
                RouteTarget::Redirect {
                    source_mount: Some(src),
                    ..
                } if src == mount_name => Some(path),
                _ => None,
            })
            .collect();

        for path in to_remove {
            self.trie.remove(&path);
        }
    }

    /// List all redirects.
    pub fn list_redirects(&self) -> Vec<(Path, Path, RedirectMode)> {
        self.trie
            .iter()
            .filter_map(|(from, target)| match target {
                RouteTarget::Redirect { target, mode, .. } => Some((from, target.clone(), *mode)),
                _ => None,
            })
            .collect()
    }

    /// Check if any store would handle this path (fallthrough).
    pub fn has_route(&self, path: &Path) -> bool {
        self.trie.find_ancestor(path).is_some()
    }

    /// Number of mounted routes (stores + redirects).
    pub fn route_count(&self) -> usize {
        self.trie.len()
    }

    /// Number of mounted stores (excluding redirects).
    pub fn store_count(&self) -> usize {
        self.trie
            .iter()
            .filter(|(_, t)| matches!(t, RouteTarget::Store(_)))
            .count()
    }

    /// True if no routes mounted.
    pub fn is_empty(&self) -> bool {
        self.trie.is_empty()
    }

    /// Iterate over all store mount points (excluding redirects).
    pub fn mounts(&self) -> impl Iterator<Item = (Path, &StoreBox)> {
        self.trie.iter().filter_map(|(path, target)| match target {
            RouteTarget::Store(s) => Some((path, s)),
            _ => None,
        })
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

/// Result of resolving a path through redirects.
struct ResolvedRoute<'a> {
    store: &'a mut StoreBox,
    suffix: Path,
    /// The prefix that led to this store (for reconstructing full paths).
    prefix: Path,
}

impl OverlayStore {
    /// Resolve a path for reading, following redirects with cycle detection.
    fn resolve_for_read(&mut self, path: &Path) -> Result<Option<ResolvedRoute<'_>>, Error> {
        let mut visited = HashSet::new();
        self.resolve_with_tracking(path, &mut visited, false)
    }

    /// Resolve a path for writing, following redirects with cycle detection.
    fn resolve_for_write(&mut self, path: &Path) -> Result<Option<ResolvedRoute<'_>>, Error> {
        let mut visited = HashSet::new();
        self.resolve_with_tracking(path, &mut visited, true)
    }

    fn resolve_with_tracking(
        &mut self,
        path: &Path,
        visited: &mut HashSet<Path>,
        is_write: bool,
    ) -> Result<Option<ResolvedRoute<'_>>, Error> {
        // Cycle detection - use the path prefix that matched, not the full path
        if !visited.insert(path.clone()) {
            return Err(Error::store(
                "overlay",
                if is_write { "write" } else { "read" },
                "redirect cycle detected",
            ));
        }

        // Find the ancestor route
        let ancestor = self.trie.find_ancestor(path);
        let (prefix_len, suffix, is_redirect, redirect_info) = match ancestor {
            Some((target, suffix)) => {
                let prefix_len = path.len() - suffix.len();
                match target {
                    RouteTarget::Store(_) => (prefix_len, suffix, false, None),
                    RouteTarget::Redirect { target, mode, .. } => {
                        // Check access mode
                        let allowed = !matches!(
                            (is_write, mode),
                            (false, RedirectMode::WriteOnly) | (true, RedirectMode::ReadOnly)
                        );
                        if !allowed {
                            return Ok(None);
                        }
                        (prefix_len, suffix, true, Some(target.clone()))
                    }
                }
            }
            None => return Ok(None),
        };

        // If it's a redirect, follow it recursively
        if is_redirect {
            if let Some(redirect_target) = redirect_info {
                let new_path = redirect_target.join(&suffix);
                return self.resolve_with_tracking(&new_path, visited, is_write);
            }
        }

        // It's a store - get mutable reference
        match self.trie.find_ancestor_mut(path) {
            Some((RouteTarget::Store(store), suffix)) => {
                let prefix = Path {
                    components: path.components[..prefix_len].to_vec(),
                };
                Ok(Some(ResolvedRoute {
                    store,
                    suffix,
                    prefix,
                }))
            }
            _ => Ok(None),
        }
    }
}

impl Reader for OverlayStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match self.resolve_for_read(from)? {
            Some(resolved) => resolved.store.read(&resolved.suffix),
            None => Err(Error::NoRoute { path: from.clone() }),
        }
    }
}

impl Writer for OverlayStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        match self.resolve_for_write(to)? {
            Some(resolved) => {
                let result_suffix = resolved.store.write(&resolved.suffix, data)?;
                Ok(resolved.prefix.join(&result_suffix))
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

    // === Redirect tests ===

    #[test]
    fn redirect_basic() {
        let mut overlay = OverlayStore::new();

        // Mount a store at /data
        let mut store = TestStore::new("data");
        store
            .write(&path!("key"), Record::parsed(Value::from("value")))
            .unwrap();
        overlay.mount(path!("data"), store);

        // Add redirect: /alias -> /data
        overlay.add_redirect(path!("alias"), path!("data"), RedirectMode::ReadWrite, None);

        // Reading /alias/key should follow redirect to /data/key
        let result = overlay.read(&path!("alias/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn redirect_write() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));

        // Add redirect: /alias -> /data
        overlay.add_redirect(path!("alias"), path!("data"), RedirectMode::ReadWrite, None);

        // Write via redirect
        overlay
            .write(&path!("alias/key"), Record::parsed(Value::from("written")))
            .unwrap();

        // Read via original path
        let result = overlay.read(&path!("data/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("written"));
    }

    #[test]
    fn redirect_read_only() {
        let mut overlay = OverlayStore::new();

        let mut store = TestStore::new("data");
        store
            .write(&path!("key"), Record::parsed(Value::from("value")))
            .unwrap();
        overlay.mount(path!("data"), store);

        // Add read-only redirect
        overlay.add_redirect(
            path!("readonly"),
            path!("data"),
            RedirectMode::ReadOnly,
            None,
        );

        // Reading works
        let result = overlay.read(&path!("readonly/key")).unwrap();
        assert!(result.is_some());

        // Writing fails (returns no route)
        let result = overlay.write(&path!("readonly/key"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn redirect_write_only() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));

        // Add write-only redirect
        overlay.add_redirect(
            path!("writeonly"),
            path!("data"),
            RedirectMode::WriteOnly,
            None,
        );

        // Writing works
        overlay
            .write(
                &path!("writeonly/key"),
                Record::parsed(Value::from("value")),
            )
            .unwrap();

        // Reading fails (returns no route)
        let result = overlay.read(&path!("writeonly/key"));
        assert!(result.is_err());

        // But original path still works
        let result = overlay.read(&path!("data/key")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn redirect_cycle_detection() {
        let mut overlay = OverlayStore::new();

        // Create a cycle: /a -> /b -> /a
        overlay.add_redirect(path!("a"), path!("b"), RedirectMode::ReadWrite, None);
        overlay.add_redirect(path!("b"), path!("a"), RedirectMode::ReadWrite, None);

        // Attempting to resolve should detect the cycle
        let result = overlay.read(&path!("a/key"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn redirect_chain() {
        let mut overlay = OverlayStore::new();

        let mut store = TestStore::new("data");
        store
            .write(&path!("key"), Record::parsed(Value::from("chained")))
            .unwrap();
        overlay.mount(path!("data"), store);

        // Create a chain: /a -> /b -> /data
        overlay.add_redirect(path!("b"), path!("data"), RedirectMode::ReadWrite, None);
        overlay.add_redirect(path!("a"), path!("b"), RedirectMode::ReadWrite, None);

        // Reading through the chain works
        let result = overlay.read(&path!("a/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("chained"));
    }

    #[test]
    fn redirect_remove_for_mount() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));

        // Add redirects with source_mount
        overlay.add_redirect(
            path!("alias1"),
            path!("data"),
            RedirectMode::ReadWrite,
            Some("mymount".to_string()),
        );
        overlay.add_redirect(
            path!("alias2"),
            path!("data"),
            RedirectMode::ReadWrite,
            Some("mymount".to_string()),
        );
        overlay.add_redirect(
            path!("other"),
            path!("data"),
            RedirectMode::ReadWrite,
            Some("othermount".to_string()),
        );

        assert_eq!(overlay.list_redirects().len(), 3);

        // Remove redirects for "mymount"
        overlay.remove_redirects_for_mount("mymount");

        // Only "other" redirect remains
        let redirects = overlay.list_redirects();
        assert_eq!(redirects.len(), 1);
        assert_eq!(redirects[0].0, path!("other"));
    }

    #[test]
    fn redirect_list() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));

        overlay.add_redirect(path!("alias"), path!("data"), RedirectMode::ReadOnly, None);

        let redirects = overlay.list_redirects();
        assert_eq!(redirects.len(), 1);
        assert_eq!(redirects[0].0, path!("alias"));
        assert_eq!(redirects[0].1, path!("data"));
        assert_eq!(redirects[0].2, RedirectMode::ReadOnly);
    }

    #[test]
    fn route_count_vs_store_count() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), TestStore::new("data"));
        overlay.add_redirect(path!("alias"), path!("data"), RedirectMode::ReadWrite, None);

        // route_count includes both stores and redirects
        assert_eq!(overlay.route_count(), 2);
        // store_count only counts stores
        assert_eq!(overlay.store_count(), 1);
    }
}
