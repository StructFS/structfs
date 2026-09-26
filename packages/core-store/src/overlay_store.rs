//! OverlayStore: Route reads/writes to different stores based on path prefixes.
//!
//! This implementation uses a prefix trie for efficient routing with fallthrough
//! semantics - the deepest matching prefix handles the request.
//!
//! Supports both direct store mounts and redirects (path aliases) with cycle detection.

use std::collections::HashSet;

use crate::path_trie::PathTrie;
use crate::{Error, Path, Reader, Record, Writer};

pub use crate::Store;

/// A boxed store that is Send + Sync.
pub type StoreBox = Box<dyn Store + Send + Sync>;

/// Access control for redirects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RedirectMode {
    /// Allow reads through this redirect.
    ReadOnly,
    /// Allow writes through this redirect.
    WriteOnly,
    /// Allow both reads and writes.
    ReadWrite,
}

/// What a route points to.
pub(crate) enum RouteTarget {
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

/// Route reads and writes to different stores based on path prefixes.
///
/// Uses a prefix trie internally for efficient routing. When a path is accessed,
/// the store walks the trie to find the deepest mounted store that matches the
/// path prefix, then delegates to that store with the remaining suffix.
///
/// Supports redirects (path aliases) that forward requests to other paths,
/// with cycle detection to prevent infinite loops.
///
/// Every `Reader` verb routes the same way: `read`, `read_children` and
/// `read_children_page` all reach the mounted store with the prefix
/// stripped, so names-only discovery is served by the store that owns the
/// data rather than projected from a full read above the routing layer.
///
/// A path that no mount covers is `Err(Error::NoRoute)`, not `Ok(None)`;
/// see [`Reader::read`] for how that interacts with layering combinators.
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
        self.mount_boxed(path, Box::new(store))
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

    /// Add a redirect from one path to another.
    ///
    /// When a path under `from` is accessed, it will be redirected to
    /// the corresponding path under `to`. An operation the `mode` does not
    /// allow (a write through a `ReadOnly` redirect, a read through a
    /// `WriteOnly` one) fails with `Error::PermissionDenied`.
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

    /// List all redirects as `(from, to, mode)`.
    pub fn list_redirects(&self) -> Vec<(Path, Path, RedirectMode)> {
        self.trie
            .iter()
            .filter_map(|(from, target)| match target {
                RouteTarget::Redirect { target, mode, .. } => Some((from, target.clone(), *mode)),
                _ => None,
            })
            .collect()
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
    fn resolve_for_read(&mut self, path: &Path) -> Result<ResolvedRoute<'_>, Error> {
        let mut visited = HashSet::new();
        self.resolve_with_tracking(path, &mut visited, false)
    }

    /// Resolve a path for writing, following redirects with cycle detection.
    fn resolve_for_write(&mut self, path: &Path) -> Result<ResolvedRoute<'_>, Error> {
        let mut visited = HashSet::new();
        self.resolve_with_tracking(path, &mut visited, true)
    }

    fn resolve_with_tracking(
        &mut self,
        path: &Path,
        visited: &mut HashSet<Path>,
        is_write: bool,
    ) -> Result<ResolvedRoute<'_>, Error> {
        let operation = if is_write { "write" } else { "read" };

        // Cycle detection - use the path prefix that matched, not the full path
        if !visited.insert(path.clone()) {
            return Err(Error::store(
                "overlay",
                operation,
                "redirect cycle detected",
            ));
        }

        // Find the ancestor route
        let Some((target, suffix)) = self.trie.find_ancestor(path) else {
            return Err(Error::NoRoute { path: path.clone() });
        };
        let prefix_len = path.len() - suffix.len();
        match target {
            RouteTarget::Store(_) => {}
            RouteTarget::Redirect { target, mode, .. } => {
                let allowed = !matches!(
                    (is_write, mode),
                    (false, RedirectMode::WriteOnly) | (true, RedirectMode::ReadOnly)
                );
                if !allowed {
                    return Err(Error::permission_denied(format!(
                        "redirect at {} does not allow {}",
                        path.slice(0, prefix_len),
                        operation
                    )));
                }
                let new_path = target.join(&suffix);
                return self.resolve_with_tracking(&new_path, visited, is_write);
            }
        }

        // It's a store - get mutable reference
        match self.trie.find_ancestor_mut(path) {
            Some((RouteTarget::Store(store), suffix)) => Ok(ResolvedRoute {
                store,
                suffix,
                prefix: path.slice(0, prefix_len),
            }),
            _ => Err(Error::NoRoute { path: path.clone() }),
        }
    }
}

impl Reader for OverlayStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let resolved = self.resolve_for_read(from)?;
        resolved.store.read(&resolved.suffix)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        let resolved = self.resolve_for_read(from)?;
        resolved.store.read_children(&resolved.suffix)
    }

    fn read_children_page(
        &mut self,
        from: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Option<crate::ChildPage>, Error> {
        let resolved = self.resolve_for_read(from)?;
        resolved
            .store
            .read_children_page(&resolved.suffix, offset, limit)
    }
}

impl Writer for OverlayStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let resolved = self.resolve_for_write(to)?;
        let result_suffix = resolved.store.write(&resolved.suffix, data)?;
        Ok(resolved.prefix.join(&result_suffix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, MemoryStore, ReadOnly, Value};

    fn store_with(path: &Path, value: Value) -> MemoryStore {
        let mut store = MemoryStore::new();
        store.write(path, Record::parsed(value)).unwrap();
        store
    }

    #[test]
    fn basic_routing() {
        let mut overlay = OverlayStore::new();

        overlay.mount(
            path!("users"),
            store_with(&path!("alice"), Value::from("Alice")),
        );
        overlay.mount(
            path!("config"),
            store_with(&path!("theme"), Value::from("dark")),
        );

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

        let store1 = store_with(&path!("key"), Value::from("first"));
        let store2 = store_with(&path!("key"), Value::from("second"));

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
        overlay.mount(path!(""), store_with(&path!("test"), Value::from("value")));

        let record = overlay.read(&path!("test")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn no_route_error() {
        let mut overlay = OverlayStore::new();

        let result = overlay.read(&path!("nonexistent"));
        assert!(matches!(result, Err(Error::NoRoute { .. })));
        assert!(matches!(
            overlay.read_children(&path!("nonexistent")),
            Err(Error::NoRoute { .. })
        ));
        assert!(matches!(
            overlay.read_children_page(&path!("nonexistent"), 0, 1),
            Err(Error::NoRoute { .. })
        ));
    }

    #[test]
    fn write_through_overlay() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), MemoryStore::new());

        let result = overlay.write(&path!("data/key"), Record::parsed(Value::from("value")));
        assert!(result.is_ok());

        let record = overlay.read(&path!("data/key")).unwrap().unwrap();
        let value = record.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn read_only_mount_rejects_writes() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("readonly"), ReadOnly::new(MemoryStore::new()));

        let result = overlay.write(&path!("readonly/key"), Record::parsed(Value::from("value")));
        assert!(matches!(result, Err(Error::PermissionDenied { .. })));
    }

    #[test]
    fn store_count() {
        let mut overlay = OverlayStore::new();
        assert_eq!(overlay.store_count(), 0);

        overlay.mount(path!("a"), MemoryStore::new());
        assert_eq!(overlay.store_count(), 1);

        overlay.mount(path!("b"), MemoryStore::new());
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
    fn write_returns_full_path() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("prefix"), MemoryStore::new());

        // Write returns the full path including the prefix
        let result = overlay.write(&path!("prefix/key"), Record::parsed(Value::from("data")));
        assert_eq!(result.unwrap(), path!("prefix/key"));
    }

    #[test]
    fn nested_prefix() {
        let mut overlay = OverlayStore::new();
        overlay.mount(
            path!("a/b"),
            store_with(&path!("deep/key"), Value::from("value")),
        );

        let result = overlay.read(&path!("a/b/deep/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("value"));
    }

    #[test]
    fn unmount_existing() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("a"), MemoryStore::new());
        overlay.mount(path!("b"), MemoryStore::new());
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
        overlay.mount(path!("a"), MemoryStore::new());

        // Try to unmount non-existent
        let removed = overlay.unmount(&path!("nonexistent"));
        assert!(removed.is_none());
        assert_eq!(overlay.store_count(), 1);
    }

    #[test]
    fn unmount_keeps_children() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), MemoryStore::new());
        overlay.mount(path!("data/nested"), MemoryStore::new());
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
    fn deeper_mount_wins() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), MemoryStore::new());
        overlay.mount(path!("data/special"), MemoryStore::new());

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
    fn is_empty() {
        let mut overlay = OverlayStore::new();
        assert!(overlay.is_empty());

        overlay.mount(path!("a"), MemoryStore::new());
        assert!(!overlay.is_empty());

        overlay.unmount(&path!("a"));
        assert!(overlay.is_empty());
    }

    #[test]
    fn mount_boxed() {
        let mut overlay = OverlayStore::new();
        let store: StoreBox = Box::new(MemoryStore::new());
        overlay.mount_boxed(path!("boxed"), store);

        overlay
            .write(&path!("boxed/key"), Record::parsed(Value::from("value")))
            .unwrap();
        let result = overlay.read(&path!("boxed/key")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn fallthrough_routing() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), MemoryStore::new());

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
        overlay.mount(path!(""), MemoryStore::new());

        // Everything routes to root store
        overlay
            .write(&path!("any/path"), Record::parsed(Value::from("v")))
            .unwrap();
        let result = overlay.read(&path!("any/path")).unwrap();
        assert!(result.is_some());
    }

    // === Discovery verbs ===

    /// Store that answers discovery from its own index without ever serving
    /// a readable record, so any fallback to `read` + projection is visible.
    struct IndexOnly;

    impl Reader for IndexOnly {
        fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
            panic!("discovery must not fall back to read")
        }

        fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
            Ok(Some(vec![format!("child_of_{}", from.len())]))
        }

        fn read_children_page(
            &mut self,
            from: &Path,
            offset: usize,
            limit: usize,
        ) -> Result<Option<crate::ChildPage>, Error> {
            let _ = from;
            Ok(Some(crate::ChildPage::new(
                vec![format!("page_{offset}_{limit}")],
                None,
            )))
        }
    }

    impl Writer for IndexOnly {
        fn write(&mut self, to: &Path, _data: Record) -> Result<Path, Error> {
            Ok(to.clone())
        }
    }

    #[test]
    fn read_children_forwards_to_routed_store_with_prefix_stripped() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("idx"), IndexOnly);

        assert_eq!(
            overlay.read_children(&path!("idx/a/b")).unwrap(),
            Some(vec!["child_of_2".to_string()])
        );
        let page = overlay
            .read_children_page(&path!("idx/a"), 3, 7)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["page_3_7".to_string()]);
    }

    #[test]
    fn read_children_forwards_through_redirect_and_root_mount() {
        let mut overlay = OverlayStore::new();
        let mut root = MemoryStore::new();
        root.write(&path!("users/alice"), Record::parsed(Value::from(1i64)))
            .unwrap();
        root.write(&path!("users/bob"), Record::parsed(Value::from(2i64)))
            .unwrap();
        overlay.mount(path!(""), root);
        overlay.add_redirect(path!("alias"), path!("users"), RedirectMode::ReadOnly, None);

        // Root mount: no prefix to strip.
        assert_eq!(
            overlay.read_children(&path!("users")).unwrap(),
            Some(vec!["alice".to_string(), "bob".to_string()])
        );
        // Redirect: `alias` resolves to `users` before reaching the store.
        assert_eq!(
            overlay.read_children(&path!("alias")).unwrap(),
            Some(vec!["alice".to_string(), "bob".to_string()])
        );
        let page = overlay
            .read_children_page(&path!("alias"), 1, 1)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["bob".to_string()]);
        assert_eq!(page.next, None);
        // Missing under a routed store is None, not NoRoute.
        assert_eq!(overlay.read_children(&path!("nothing")).unwrap(), None);
    }

    // === Redirect tests ===

    #[test]
    fn redirect_basic() {
        let mut overlay = OverlayStore::new();

        // Mount a store at /data
        overlay.mount(
            path!("data"),
            store_with(&path!("key"), Value::from("value")),
        );

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
        overlay.mount(path!("data"), MemoryStore::new());

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
    fn redirect_read_only_denies_writes() {
        let mut overlay = OverlayStore::new();
        overlay.mount(
            path!("data"),
            store_with(&path!("key"), Value::from("value")),
        );

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

        // Writing is denied — explicitly, not reported as a missing route.
        let result = overlay.write(&path!("readonly/key"), Record::parsed(Value::Null));
        assert!(matches!(result, Err(Error::PermissionDenied { .. })));
    }

    #[test]
    fn redirect_write_only_denies_reads() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), MemoryStore::new());

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

        // Reading (any verb) is denied
        assert!(matches!(
            overlay.read(&path!("writeonly/key")),
            Err(Error::PermissionDenied { .. })
        ));
        assert!(matches!(
            overlay.read_children(&path!("writeonly")),
            Err(Error::PermissionDenied { .. })
        ));

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
        overlay.mount(
            path!("data"),
            store_with(&path!("key"), Value::from("chained")),
        );

        // Create a chain: /a -> /b -> /data
        overlay.add_redirect(path!("b"), path!("data"), RedirectMode::ReadWrite, None);
        overlay.add_redirect(path!("a"), path!("b"), RedirectMode::ReadWrite, None);

        // Reading through the chain works
        let result = overlay.read(&path!("a/key")).unwrap().unwrap();
        let value = result.into_value(&crate::NoCodec).unwrap();
        assert_eq!(value, Value::from("chained"));
    }

    #[test]
    fn redirect_to_unmounted_target_is_no_route() {
        let mut overlay = OverlayStore::new();
        overlay.add_redirect(path!("alias"), path!("gone"), RedirectMode::ReadWrite, None);
        assert!(matches!(
            overlay.read(&path!("alias/key")),
            Err(Error::NoRoute { .. })
        ));
    }

    #[test]
    fn redirect_remove_for_mount() {
        let mut overlay = OverlayStore::new();
        overlay.mount(path!("data"), MemoryStore::new());

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
        overlay.mount(path!("data"), MemoryStore::new());

        overlay.add_redirect(path!("alias"), path!("data"), RedirectMode::ReadOnly, None);

        let redirects = overlay.list_redirects();
        assert_eq!(redirects.len(), 1);
        assert_eq!(redirects[0].0, path!("alias"));
        assert_eq!(redirects[0].1, path!("data"));
        assert_eq!(redirects[0].2, RedirectMode::ReadOnly);
        // Redirects are not stores.
        assert_eq!(overlay.store_count(), 1);
    }
}
