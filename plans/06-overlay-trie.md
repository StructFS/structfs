# Plan 6: OverlayStore with Prefix Trie

## Problem

Current `OverlayStore` uses a `Vec<(Path, StoreBox)>` which:

1. Allows duplicate prefixes (no use case, just a bug waiting to happen)
2. Requires linear scan with priority rules ("last added wins")
3. Makes `remove_layer` semantics unclear (remove one? all matching?)
4. Doesn't represent the routing structure - it *encodes* it in traversal logic

```rust
// Current: flat list with implicit priority
struct OverlayStore {
    routes: Vec<(Path, StoreBox)>,
}
```

## Solution

Two components with clear separation of concerns:

### 1. Generic Prefix Trie (`PathTrie<T>`)

A reusable data structure for path-keyed storage:

```rust
/// A prefix trie keyed by path components.
/// Each node can hold a value and has children indexed by path component.
pub struct PathTrie<T> {
    value: Option<T>,
    children: BTreeMap<String, PathTrie<T>>,
}
```

This is a pure data structure with no domain knowledge of stores, routing, or StructFS.

### 2. OverlayStore wraps `PathTrie<StoreBox>`

```rust
pub struct OverlayStore {
    trie: PathTrie<StoreBox>,
}
```

OverlayStore provides the domain-specific routing semantics (fallthrough, Reader/Writer impl) on top of the generic trie.

Benefits:
- Clean separation: trie handles structure, OverlayStore handles semantics
- Trie is reusable for other path-indexed data (config, metadata, etc.)
- No duplicate prefixes possible by construction
- Subtree operations are natural on the trie
- Easier to test each layer independently

## Routing Semantics (Fallthrough)

When reading/writing to path `/a/b/c/key`:

1. Walk the trie: root → `a` → `b` → `c` → (no `key` child)
2. Track the **deepest node with a store** during traversal
3. Delegate to that store with the **remaining path suffix**

Example:
```
root
├── data/          [store: DataStore]
│   └── cache/     [store: CacheStore]
└── config/        [store: ConfigStore]
```

| Path | Routed To | Suffix |
|------|-----------|--------|
| `/data/users/1` | DataStore | `users/1` |
| `/data/cache/hot` | CacheStore | `hot` |
| `/data/cache` | CacheStore | `` (empty) |
| `/config/app.json` | ConfigStore | `app.json` |
| `/unknown/path` | None (NoRoute error) | - |

This matches current behavior but makes it explicit in the structure.

## API Design

### PathTrie<T> - Generic Prefix Trie

```rust
impl<T> PathTrie<T> {
    /// Create an empty trie
    pub fn new() -> Self;

    /// Insert a value at path. Returns previous value if any.
    pub fn insert(&mut self, path: &Path, value: T) -> Option<T>;

    /// Remove and return value at exact path. Children remain.
    pub fn remove(&mut self, path: &Path) -> Option<T>;

    /// Remove and return entire subtree at path.
    pub fn remove_subtree(&mut self, path: &Path) -> Option<PathTrie<T>>;

    /// Get reference to value at exact path.
    pub fn get(&self, path: &Path) -> Option<&T>;

    /// Get mutable reference to value at exact path.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut T>;

    /// Get reference to subtrie at path.
    pub fn get_subtrie(&self, path: &Path) -> Option<&PathTrie<T>>;

    /// Get mutable reference to subtrie at path.
    pub fn get_subtrie_mut(&mut self, path: &Path) -> Option<&mut PathTrie<T>>;

    /// Check if path exists (has value or children)
    pub fn contains(&self, path: &Path) -> bool;

    /// Check if exact path has a value
    pub fn contains_value(&self, path: &Path) -> bool;

    /// Count of values in trie (not nodes)
    pub fn len(&self) -> usize;

    /// True if no values anywhere in trie
    pub fn is_empty(&self) -> bool;

    /// Iterate over all (path, value) pairs
    pub fn iter(&self) -> impl Iterator<Item = (Path, &T)>;

    /// Iterate mutably over all (path, value) pairs
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Path, &mut T)>;

    /// Find deepest ancestor with a value (for fallthrough routing)
    /// Returns (value, remaining_suffix)
    pub fn find_ancestor(&self, path: &Path) -> Option<(&T, Path)>;

    /// Mutable version of find_ancestor
    pub fn find_ancestor_mut(&mut self, path: &Path) -> Option<(&mut T, Path)>;
}
```

### OverlayStore - Domain Wrapper

```rust
impl OverlayStore {
    /// Create an empty overlay
    pub fn new() -> Self;

    /// Mount a store at path. Returns previous store if any.
    pub fn mount<S: Store + Send + Sync + 'static>(
        &mut self,
        path: Path,
        store: S,
    ) -> Option<StoreBox>;

    /// Mount a boxed store
    pub fn mount_boxed(&mut self, path: Path, store: StoreBox) -> Option<StoreBox>;

    /// Unmount store at exact path, keeping any nested mounts.
    pub fn unmount(&mut self, path: &Path) -> Option<StoreBox>;

    /// Unmount entire subtree, returning it as a new OverlayStore.
    pub fn unmount_subtree(&mut self, path: &Path) -> Option<OverlayStore>;

    /// Check if any store would handle this path (fallthrough)
    pub fn has_route(&self, path: &Path) -> bool;

    /// Number of mounted stores
    pub fn store_count(&self) -> usize;

    /// True if no stores mounted
    pub fn is_empty(&self) -> bool;

    /// Iterate over all mount points
    pub fn mounts(&self) -> impl Iterator<Item = (Path, &StoreBox)>;
}
```

### Why Two Unmount Methods?

`unmount(&Path) -> Option<StoreBox>`:
- Removes only the store at that exact path
- Children remain (they may have their own stores)
- Use when replacing a store but keeping nested mounts

`unmount_subtree(&Path) -> Option<OverlayStore>`:
- Removes entire subtree
- Use when tearing down a mount point completely
- Returned subtree can be re-mounted elsewhere or inspected

Example:
```rust
// Setup: /data has DataStore, /data/cache has CacheStore
overlay.mount(path!("data"), DataStore::new());
overlay.mount(path!("data/cache"), CacheStore::new());

// unmount("data") removes DataStore but keeps /data/cache
let data_store = overlay.unmount(&path!("data"));
// /data/cache still works!

// unmount_subtree("data") would remove both DataStore AND CacheStore
let subtree = overlay.unmount_subtree(&path!("data"));
// subtree contains both stores
```

## Implementation

### PathTrie<T> - New File: `path_trie.rs`

```rust
use crate::Path;
use std::collections::BTreeMap;

/// A prefix trie keyed by path components.
///
/// Each node can optionally hold a value of type T, and has children
/// indexed by path component strings. This provides O(k) operations
/// where k is the path depth.
///
/// # Example
///
/// ```rust
/// use structfs_core_store::{PathTrie, path};
///
/// let mut trie: PathTrie<i32> = PathTrie::new();
/// trie.insert(&path!("a/b"), 1);
/// trie.insert(&path!("a/b/c"), 2);
///
/// assert_eq!(trie.get(&path!("a/b")), Some(&1));
/// assert_eq!(trie.find_ancestor(&path!("a/b/c/d")), Some((&2, path!("d"))));
/// ```
#[derive(Debug, Clone)]
pub struct PathTrie<T> {
    value: Option<T>,
    children: BTreeMap<String, PathTrie<T>>,
}

impl<T> Default for PathTrie<T> {
    fn default() -> Self {
        Self {
            value: None,
            children: BTreeMap::new(),
        }
    }
}

impl<T> PathTrie<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Navigate to node, creating intermediate nodes as needed.
    fn get_or_create_node(&mut self, path: &Path) -> &mut PathTrie<T> {
        let mut current = self;
        for component in &path.components {
            current = current.children
                .entry(component.clone())
                .or_insert_with(PathTrie::new);
        }
        current
    }

    /// Navigate to node if it exists.
    fn get_node(&self, path: &Path) -> Option<&PathTrie<T>> {
        let mut current = self;
        for component in &path.components {
            current = current.children.get(component)?;
        }
        Some(current)
    }

    /// Navigate to node if it exists (mutable).
    fn get_node_mut(&mut self, path: &Path) -> Option<&mut PathTrie<T>> {
        let mut current = self;
        for component in &path.components {
            current = current.children.get_mut(component)?;
        }
        Some(current)
    }

    /// Insert value at path. Returns previous value if any.
    pub fn insert(&mut self, path: &Path, value: T) -> Option<T> {
        let node = self.get_or_create_node(path);
        node.value.replace(value)
    }

    /// Remove and return value at exact path. Children remain.
    pub fn remove(&mut self, path: &Path) -> Option<T> {
        self.get_node_mut(path)?.value.take()
    }

    /// Remove and return entire subtree at path.
    pub fn remove_subtree(&mut self, path: &Path) -> Option<PathTrie<T>> {
        if path.is_empty() {
            let old = std::mem::take(self);
            if old.value.is_some() || !old.children.is_empty() {
                Some(old)
            } else {
                None
            }
        } else {
            let parent_path = Path {
                components: path.components[..path.len() - 1].to_vec(),
            };
            let child_name = &path.components[path.len() - 1];
            let parent = self.get_node_mut(&parent_path)?;
            parent.children.remove(child_name)
        }
    }

    /// Get reference to value at exact path.
    pub fn get(&self, path: &Path) -> Option<&T> {
        self.get_node(path)?.value.as_ref()
    }

    /// Get mutable reference to value at exact path.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut T> {
        self.get_node_mut(path)?.value.as_mut()
    }

    /// Get reference to subtrie at path.
    pub fn get_subtrie(&self, path: &Path) -> Option<&PathTrie<T>> {
        self.get_node(path)
    }

    /// Get mutable reference to subtrie at path.
    pub fn get_subtrie_mut(&mut self, path: &Path) -> Option<&mut PathTrie<T>> {
        self.get_node_mut(path)
    }

    /// Check if exact path has a value.
    pub fn contains_value(&self, path: &Path) -> bool {
        self.get(path).is_some()
    }

    /// Count of values in trie.
    pub fn len(&self) -> usize {
        let self_count = if self.value.is_some() { 1 } else { 0 };
        let children_count: usize = self.children.values()
            .map(|child| child.len())
            .sum();
        self_count + children_count
    }

    /// True if no values anywhere in trie.
    pub fn is_empty(&self) -> bool {
        self.value.is_none() && self.children.values().all(|c| c.is_empty())
    }

    /// Find deepest ancestor with a value.
    /// Returns (value_ref, remaining_suffix).
    pub fn find_ancestor(&self, path: &Path) -> Option<(&T, Path)> {
        let mut current = self;
        let mut last_value: Option<&T> = self.value.as_ref();
        let mut last_depth: usize = 0;

        for (depth, component) in path.components.iter().enumerate() {
            match current.children.get(component) {
                Some(child) => {
                    current = child;
                    if child.value.is_some() {
                        last_value = child.value.as_ref();
                        last_depth = depth + 1;
                    }
                }
                None => break,
            }
        }

        last_value.map(|v| {
            let suffix = Path {
                components: path.components[last_depth..].to_vec(),
            };
            (v, suffix)
        })
    }

    /// Mutable version of find_ancestor.
    /// Due to borrow checker constraints, this uses a two-pass approach.
    pub fn find_ancestor_mut(&mut self, path: &Path) -> Option<(&mut T, Path)> {
        // First pass: find the depth
        let depth = {
            let mut current = &*self;
            let mut last_depth: usize = if self.value.is_some() { 0 } else { usize::MAX };

            for (d, component) in path.components.iter().enumerate() {
                match current.children.get(component) {
                    Some(child) => {
                        current = child;
                        if child.value.is_some() {
                            last_depth = d + 1;
                        }
                    }
                    None => break,
                }
            }

            if last_depth == usize::MAX {
                return None;
            }
            last_depth
        };

        // Second pass: get mutable reference
        let target_path = Path {
            components: path.components[..depth].to_vec(),
        };
        let suffix = Path {
            components: path.components[depth..].to_vec(),
        };

        self.get_mut(&target_path).map(|v| (v, suffix))
    }

    /// Iterate over all (path, value) pairs.
    pub fn iter(&self) -> PathTrieIter<'_, T> {
        PathTrieIter::new(self)
    }
}

/// Iterator over (Path, &T) pairs in a PathTrie.
pub struct PathTrieIter<'a, T> {
    stack: Vec<(Path, &'a PathTrie<T>)>,
}

impl<'a, T> PathTrieIter<'a, T> {
    fn new(trie: &'a PathTrie<T>) -> Self {
        Self {
            stack: vec![(Path::empty(), trie)],
        }
    }
}

impl<'a, T> Iterator for PathTrieIter<'a, T> {
    type Item = (Path, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((path, node)) = self.stack.pop() {
            // Push children onto stack (in reverse order for correct iteration)
            for (name, child) in node.children.iter().rev() {
                self.stack.push((path.join(&Path::parse(name).unwrap()), child));
            }

            // Yield this node if it has a value
            if let Some(ref value) = node.value {
                return Some((path, value));
            }
        }
        None
    }
}
```

### OverlayStore - Wraps PathTrie

```rust
use crate::{path_trie::PathTrie, Error, Path, Reader, Record, Writer};

pub type StoreBox = Box<dyn Store + Send + Sync>;

pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}

pub struct OverlayStore {
    trie: PathTrie<StoreBox>,
}

impl Default for OverlayStore {
    fn default() -> Self {
        Self {
            trie: PathTrie::new(),
        }
    }
}

impl OverlayStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mount<S: Store + Send + Sync + 'static>(
        &mut self,
        path: Path,
        store: S,
    ) -> Option<StoreBox> {
        self.trie.insert(&path, Box::new(store))
    }

    pub fn mount_boxed(&mut self, path: Path, store: StoreBox) -> Option<StoreBox> {
        self.trie.insert(&path, store)
    }

    pub fn unmount(&mut self, path: &Path) -> Option<StoreBox> {
        self.trie.remove(path)
    }

    pub fn unmount_subtree(&mut self, path: &Path) -> Option<OverlayStore> {
        self.trie.remove_subtree(path).map(|trie| OverlayStore { trie })
    }

    pub fn has_route(&self, path: &Path) -> bool {
        self.trie.find_ancestor(path).is_some()
    }

    pub fn store_count(&self) -> usize {
        self.trie.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trie.is_empty()
    }

    pub fn mounts(&self) -> impl Iterator<Item = (Path, &StoreBox)> {
        self.trie.iter()
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
```

## Migration from Vec-based Implementation

### Compatibility Shim

During migration, provide the old method names pointing to new ones:

```rust
impl OverlayStore {
    #[deprecated(note = "use mount() instead")]
    pub fn add_layer<S: Store + Send + Sync + 'static>(&mut self, path: Path, store: S) {
        self.mount(path, store);
    }

    #[deprecated(note = "use unmount() instead")]
    pub fn remove_layer(&mut self, path: &Path) -> Option<StoreBox> {
        self.unmount(path)
    }

    #[deprecated(note = "use store_count() instead")]
    pub fn layer_count(&self) -> usize {
        self.store_count()
    }
}
```

### Update MountStore

`MountStore` changes minimally:

```rust
// Before
self.overlay.add_layer(mount_path, store);
self.overlay.remove_layer(&mount_path);

// After
self.overlay.mount(mount_path, store);
self.overlay.unmount(&mount_path);
```

## Tests

```rust
#[test]
fn mount_creates_path() {
    let mut overlay = OverlayStore::new();
    overlay.mount(path!("a/b/c"), TestStore::new());

    assert_eq!(overlay.store_count(), 1);
    assert!(overlay.has_route(&path!("a/b/c/anything")));
}

#[test]
fn mount_replaces_existing() {
    let mut overlay = OverlayStore::new();
    overlay.mount(path!("data"), TestStore::new());

    let old = overlay.mount(path!("data"), TestStore::new());
    assert!(old.is_some());
    assert_eq!(overlay.store_count(), 1);
}

#[test]
fn fallthrough_routing() {
    let mut overlay = OverlayStore::new();
    overlay.mount(path!("data"), TestStore::new());

    // Write via fallthrough
    overlay.write(&path!("data/deep/nested/key"), Record::parsed(Value::from("value"))).unwrap();

    // Read via fallthrough
    let result = overlay.read(&path!("data/deep/nested/key")).unwrap();
    assert!(result.is_some());
}

#[test]
fn deeper_mount_wins() {
    let mut overlay = OverlayStore::new();
    overlay.mount(path!("data"), TestStore::new());
    overlay.mount(path!("data/special"), TestStore::new());

    // Write to data/special/key goes to the special store
    overlay.write(&path!("data/special/key"), Record::parsed(Value::from("special"))).unwrap();

    // Write to data/other/key goes to the data store
    overlay.write(&path!("data/other/key"), Record::parsed(Value::from("other"))).unwrap();

    // Unmount special, data still works
    overlay.unmount(&path!("data/special"));
    let result = overlay.read(&path!("data/other/key")).unwrap();
    assert!(result.is_some());
}

#[test]
fn unmount_subtree_removes_all() {
    let mut overlay = OverlayStore::new();
    overlay.mount(path!("data"), TestStore::new());
    overlay.mount(path!("data/cache"), TestStore::new());

    let subtree = overlay.unmount_subtree(&path!("data")).unwrap();

    assert_eq!(subtree.store_count(), 2);
    assert!(overlay.is_empty());
}

#[test]
fn unmount_keeps_children() {
    let mut overlay = OverlayStore::new();
    overlay.mount(path!("data"), TestStore::new());
    overlay.mount(path!("data/cache"), TestStore::new());

    let store = overlay.unmount(&path!("data"));
    assert!(store.is_some());

    // data/cache still works
    assert_eq!(overlay.store_count(), 1);
    overlay.write(&path!("data/cache/key"), Record::parsed(Value::from("v"))).unwrap();
}

#[test]
fn no_route_error() {
    let mut overlay = OverlayStore::new();

    let result = overlay.read(&path!("anything"));
    assert!(matches!(result, Err(Error::NoRoute { .. })));
}

#[test]
fn empty_path_mounts_at_root() {
    let mut overlay = OverlayStore::new();
    overlay.mount(Path::empty(), TestStore::new());

    // Everything routes to root store
    overlay.write(&path!("any/path"), Record::parsed(Value::from("v"))).unwrap();
    let result = overlay.read(&path!("any/path")).unwrap();
    assert!(result.is_some());
}
```

## Files Changed

- `packages/core-store/src/path_trie.rs` - New file: generic prefix trie data structure
- `packages/core-store/src/overlay_store.rs` - Rewrite to wrap PathTrie<StoreBox>
- `packages/core-store/src/mount_store.rs` - Update to use new API (mount/unmount)
- `packages/core-store/src/lib.rs` - Add `mod path_trie` and update exports

## Complexity

Medium-High - Significant rewrite of OverlayStore, but:
- API is cleaner and more intuitive
- Tests are straightforward
- MountStore changes are minimal
- No changes needed outside core-store

## Future Considerations

1. **Merge**: Combine two OverlayStores:
   ```rust
   pub fn merge(&mut self, other: OverlayStore, conflict: ConflictStrategy)
   ```

2. **Snapshot**: Clone the structure (not the stores) for debugging:
   ```rust
   pub fn structure(&self) -> OverlayStructure  // paths only, no stores
   ```

3. **Prune**: Remove empty intermediate nodes after unmount:
   ```rust
   pub fn prune(&mut self)  // clean up valueless leaves
   ```
