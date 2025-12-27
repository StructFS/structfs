# Plan 7: Docs Router and Auto-Discovery

## The Insight

Docs should live with their stores. The help system should be a **view** over docs, not a **copy** of them.

When you mount a store, discover its docs and make them visible in the help namespace. HelpStore becomes an aggregator/indexer, not a content holder.

## Decisions Made

| Question | Decision |
|----------|----------|
| Redirect implementation | Path-based in OverlayStore using PathTrie |
| Loop prevention | Visited set tracking |
| Unmount cascade | Automatic (remove redirects when store unmounts) |
| REPL help location | ReplDocsStore at /ctx/repl |
| Write through redirects | Configurable per-redirect (read-only, write-only, read-write) |
| Discovery timing | Probe at mount time only |
| List behavior | Separate endpoints (no underscore prefix) |

## Current State

The codebase now uses a `PathTrie<StoreBox>` for routing:

- `OverlayStore` wraps `PathTrie<StoreBox>` with `find_ancestor()` for fallthrough
- `mount()` and `unmount()` work correctly via trie operations
- `unmount_subtree()` removes entire subtrees
- `OnlyReadable` / `OnlyWritable` wrappers exist for access control
- `SubStoreView` exists for viewing into a sub-path of a store

## Architecture

### 1. RouteTarget Enum

Extend the trie to hold either stores or redirects:

```rust
/// Access control for redirects
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

/// What a route points to
pub enum RouteTarget {
    /// Direct store mount
    Store(StoreBox),
    /// Redirect to another path in the overlay
    Redirect {
        target: Path,
        mode: RedirectMode,
        /// Which mount created this redirect (for cascade unmount)
        source_mount: Option<String>,
    },
}

/// The overlay now stores RouteTargets instead of StoreBox directly
pub struct OverlayStore {
    trie: PathTrie<RouteTarget>,
}
```

### 2. Redirect Resolution with Cycle Detection

```rust
impl OverlayStore {
    fn resolve_read(&mut self, path: &Path) -> Result<Option<(&mut StoreBox, Path)>, Error> {
        let mut visited = HashSet::new();
        self.resolve_with_tracking(path, &mut visited, false)
    }

    fn resolve_write(&mut self, path: &Path) -> Result<Option<(&mut StoreBox, Path)>, Error> {
        let mut visited = HashSet::new();
        self.resolve_with_tracking(path, &mut visited, true)
    }

    fn resolve_with_tracking(
        &mut self,
        path: &Path,
        visited: &mut HashSet<Path>,
        is_write: bool,
    ) -> Result<Option<(&mut StoreBox, Path)>, Error> {
        // Cycle detection
        if !visited.insert(path.clone()) {
            return Err(Error::store("overlay", "resolve", "redirect cycle detected"));
        }

        match self.trie.find_ancestor_mut(path) {
            Some((target, suffix)) => match target {
                RouteTarget::Store(store) => Ok(Some((store, suffix))),
                RouteTarget::Redirect { target: redirect_path, mode, .. } => {
                    // Check access mode
                    let allowed = match (is_write, mode) {
                        (false, RedirectMode::WriteOnly) => false,
                        (true, RedirectMode::ReadOnly) => false,
                        _ => true,
                    };
                    if !allowed {
                        return Ok(None);
                    }

                    // Follow redirect
                    let new_path = redirect_path.join(&suffix);
                    self.resolve_with_tracking(&new_path, visited, is_write)
                }
            },
            None => Ok(None),
        }
    }
}
```

### 3. Redirect Management

```rust
impl OverlayStore {
    /// Add a redirect from one path to another
    pub fn add_redirect(
        &mut self,
        from: Path,
        to: Path,
        mode: RedirectMode,
        source_mount: Option<String>,
    ) {
        self.trie.insert(&from, RouteTarget::Redirect {
            target: to,
            mode,
            source_mount,
        });
    }

    /// Remove all redirects created by a specific mount
    pub fn remove_redirects_for_mount(&mut self, mount_name: &str) {
        // Collect paths to remove (can't mutate while iterating)
        let to_remove: Vec<Path> = self.trie.iter()
            .filter_map(|(path, target)| {
                match target {
                    RouteTarget::Redirect { source_mount: Some(src), .. }
                        if src == mount_name => Some(path),
                    _ => None,
                }
            })
            .collect();

        for path in to_remove {
            self.trie.remove(&path);
        }
    }

    /// List all redirects
    pub fn list_redirects(&self) -> Vec<(Path, Path, RedirectMode)> {
        self.trie.iter()
            .filter_map(|(from, target)| {
                match target {
                    RouteTarget::Redirect { target, mode, .. } =>
                        Some((from, target.clone(), *mode)),
                    _ => None,
                }
            })
            .collect()
    }
}
```

### 4. Docs Protocol Convention

Stores that support documentation expose it at a `docs` subpath:

```
/ctx/sys/docs           # SysStore documentation root
/ctx/sys/docs/env       # Environment subsystem docs
/ctx/sys/docs/time      # Time subsystem docs

/ctx/http/docs          # HttpBrokerStore documentation
```

Reading the root docs path returns a manifest:

```json
{
  "title": "System Store",
  "description": "OS primitives exposed through paths",
  "version": "0.1.0",
  "children": ["env", "time", "random", "proc", "fs"]
}
```

### 5. Mount-Time Discovery

```rust
impl<F: StoreFactory> MountStore<F> {
    pub fn mount(&mut self, name: &str, config: MountConfig) -> Result<(), Error> {
        let store = self.factory.create(&config)?;
        let mount_path = Path::parse(name)?;

        // Add store to overlay
        self.overlay.mount(mount_path.clone(), store);
        self.mounts.insert(name.to_string(), config);

        // Discovery: check for docs at mount time
        self.discover_and_redirect_docs(name, &mount_path)?;

        Ok(())
    }

    fn discover_and_redirect_docs(&mut self, name: &str, mount_path: &Path) -> Result<(), Error> {
        let docs_path = mount_path.join(&path!("docs"));

        // Probe for docs - must exist at mount time
        if self.overlay.read(&docs_path).ok().flatten().is_some() {
            // Create redirect: /ctx/help/{name} → {mount_path}/docs
            let help_path = path!("ctx/help").join(&Path::parse(name)?);
            self.overlay.add_redirect(
                help_path,
                docs_path,
                RedirectMode::ReadOnly,
                Some(name.to_string()),
            );
        }

        Ok(())
    }

    pub fn unmount(&mut self, name: &str) -> Result<(), Error> {
        if !self.mounts.contains_key(name) {
            return Err(Error::store("mount_store", "unmount",
                format!("No mount at '{}'", name)));
        }

        let mount_path = Path::parse(name)?;

        // Remove the store from overlay
        self.overlay.unmount(&mount_path);

        // Cascade: remove any redirects this mount created
        self.overlay.remove_redirects_for_mount(name);

        // Remove from tracking
        self.mounts.remove(name);

        Ok(())
    }
}
```

### 6. HelpStore as Index/Search

HelpStore handles metadata and search, not content:

```rust
pub struct HelpStore {
    /// Index of discovered docs for search
    index: DocsIndex,
}

impl Reader for HelpStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match from.get(0).map(|s| s.as_str()) {
            // /ctx/help - list available topics
            None => Ok(Some(self.index.list_topics())),

            // /ctx/help/search/{query} - search across docs
            Some("search") => {
                let query = from.get(1).map(|s| s.as_str()).unwrap_or("");
                Ok(Some(self.index.search(query)))
            }

            // /ctx/help/meta - redirect details
            Some("meta") if from.len() == 1 => {
                Ok(Some(self.index.list_redirects()))
            }

            // /ctx/help/meta/{topic} - specific redirect info
            Some("meta") if from.len() > 1 => {
                Ok(self.index.get_redirect_info(&from[1]))
            }

            // Everything else handled by redirects in overlay
            _ => Ok(None),
        }
    }
}
```

HelpStore paths:

| Path | Returns |
|------|---------|
| `/ctx/help` | List of topic names: `["sys", "http", "repl", ...]` |
| `/ctx/help/search/{query}` | Search results across all indexed docs |
| `/ctx/help/meta` | All redirect details: `[{name, target, mode}, ...]` |
| `/ctx/help/meta/{topic}` | Single redirect info |

Content paths (`/ctx/help/sys`, etc.) are redirects handled by OverlayStore.

### 7. ReplDocsStore

REPL-specific documentation as a proper store:

```rust
pub struct ReplDocsStore {
    docs: BTreeMap<String, Value>,
}

impl ReplDocsStore {
    pub fn new() -> Self {
        let mut docs = BTreeMap::new();

        // Build docs for commands, registers, paths, examples...
        docs.insert("".to_string(), Self::root_docs());
        docs.insert("commands".to_string(), Self::commands_docs());
        docs.insert("registers".to_string(), Self::registers_docs());
        docs.insert("paths".to_string(), Self::paths_docs());
        docs.insert("examples".to_string(), Self::examples_docs());

        Self { docs }
    }

    fn root_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("REPL Documentation".into()));
        map.insert("children".into(), Value::Array(vec![
            Value::String("commands".into()),
            Value::String("registers".into()),
            Value::String("paths".into()),
            Value::String("examples".into()),
        ]));
        Value::Map(map)
    }

    // ... other doc builders
}

impl Reader for ReplDocsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Convert path to key
        let key = from.to_string();
        Ok(self.docs.get(&key).cloned().map(Record::parsed))
    }
}
```

Mount at `/ctx/repl` with docs at `/ctx/repl/docs`. Discovery creates redirect `/ctx/help/repl` → `/ctx/repl/docs`.

## The Full Flow

```
1. REPL starts
2. MountStore::mount("ctx/sys", MountConfig::Sys)
   → Creates SysStore
   → Mounts at /ctx/sys (in trie)
   → Discovery: reads /ctx/sys/docs - SUCCESS
   → Creates redirect: /ctx/help/sys → /ctx/sys/docs
   → HelpStore indexes the manifest

3. User: read /ctx/help
   → HelpStore returns list: ["sys", "http", "repl", ...]

4. User: read /ctx/help/sys
   → OverlayStore finds redirect in trie
   → Resolves to /ctx/sys/docs (with cycle detection)
   → Returns SysStore's docs

5. User: read /ctx/help/sys/env
   → Redirect + suffix → /ctx/sys/docs/env
   → Returns env-specific docs

6. User: read /ctx/help/search/time
   → HelpStore handles search directly
   → Returns matches from index

7. MountStore::unmount("ctx/sys")
   → Removes /ctx/sys from trie
   → Removes all redirects with source_mount="ctx/sys"
   → /ctx/help/sys is now gone
```

## Implementation Steps

### Step 1: Add RouteTarget to OverlayStore

Change `PathTrie<StoreBox>` to `PathTrie<RouteTarget>`. Update all methods.

### Step 2: Add redirect resolution with cycle detection

Implement `resolve_with_tracking()` using visited set.

### Step 3: Add redirect management methods

`add_redirect()`, `remove_redirects_for_mount()`, `list_redirects()`.

### Step 4: Add discovery to MountStore::mount

Probe for docs, create redirects automatically.

### Step 5: Update MountStore::unmount for cascade

Call `remove_redirects_for_mount()` on unmount.

### Step 6: Simplify HelpStore

Remove hard-coded topics. Keep only index/search/meta.

### Step 7: Create ReplDocsStore

Move REPL docs to dedicated store, mount at /ctx/repl.

### Step 8: Add DocsIndex to HelpStore

For search and topic listing.

## Files Changed

- `packages/core-store/src/overlay_store.rs` - Add RouteTarget, redirect resolution
- `packages/core-store/src/mount_store.rs` - Add discovery, cascade unmount
- `packages/repl/src/help_store.rs` - Simplify to index/search only
- `packages/repl/src/repl_docs_store.rs` - New file for REPL docs
- `packages/repl/src/store_context.rs` - Mount ReplDocsStore

## Benefits

1. **Single source of truth** - Docs live with stores, not duplicated
2. **Automatic discovery** - Mount a store, get its docs for free
3. **Consistent access** - `/ctx/help/X` always works if `/ctx/X/docs` exists
4. **Searchable** - Index enables cross-store search
5. **Extensible** - New stores automatically integrate
6. **No wiring code** - Discovery is automatic at mount time
7. **Clean unmount** - Redirects cascade-delete with their source mount
8. **Cycle-safe** - Visited set prevents infinite redirect loops
