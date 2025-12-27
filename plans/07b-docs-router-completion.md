# Plan 7b: Docs Router Completion (S-Tier)

## Current State

The redirect infrastructure is solid:
- `RouteTarget` enum with `Store` and `Redirect` variants ✓
- Cycle detection via visited set ✓
- Cascade unmount via `source_mount` tracking ✓
- Mount-time discovery for sys/http stores ✓

What's broken:
- HelpStore still has 500+ lines of hard-coded REPL docs
- No `/ctx/help` listing of available topics
- No `/ctx/help/meta` for redirect introspection
- No search functionality
- No ReplDocsStore - REPL breaks the "everything is a store" principle

## The S-Tier Vision

```
/ctx/help                     → List all available topics (from redirect table)
/ctx/help/meta                → All redirect mappings [{from, to, mode}]
/ctx/help/meta/{topic}        → Single redirect info
/ctx/help/search/{query}      → Search across all indexed docs
/ctx/help/{topic}             → REDIRECT to /{topic}/docs
/ctx/help/{topic}/{subpath}   → REDIRECT to /{topic}/docs/{subpath}

/ctx/repl                     → ReplStore (commands, state, etc.)
/ctx/repl/docs                → REPL documentation
/ctx/repl/docs/commands       → Command syntax docs
/ctx/repl/docs/registers      → Register syntax docs
/ctx/repl/docs/paths          → Path syntax docs
/ctx/repl/docs/examples       → Usage examples
```

Every piece of documentation comes from a store. HelpStore is **only** an aggregator.

## Architecture

### 1. HelpStore as Pure Aggregator

HelpStore holds NO content. It provides three services:

1. **Topic listing** - Derived from redirect table
2. **Metadata** - Expose redirect mappings
3. **Search** - Index and query across docs

```rust
pub struct HelpStore {
    /// Index for search functionality
    index: DocsIndex,
}

impl HelpStore {
    pub fn new() -> Self {
        Self {
            index: DocsIndex::new(),
        }
    }

    /// Called when a docs redirect is created
    pub fn index_docs(&mut self, topic: &str, manifest: Option<Value>) {
        self.index.add_topic(topic, manifest);
    }

    /// Called when a docs redirect is removed
    pub fn unindex_docs(&mut self, topic: &str) {
        self.index.remove_topic(topic);
    }
}

impl Reader for HelpStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            // GET /ctx/help → list all topics
            return Ok(Some(Record::parsed(self.index.list_topics())));
        }

        match from[0].as_str() {
            "meta" => self.read_meta(&from.slice(1, from.len())),
            "search" => self.read_search(&from.slice(1, from.len())),
            _ => Ok(None), // Everything else handled by redirects
        }
    }
}

impl Writer for HelpStore {
    fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::store("help", "write", "Help store is read-only"))
    }
}
```

### 2. DocsIndex

A simple index for topic listing and search:

```rust
pub struct DocsIndex {
    /// topic_name -> DocsManifest
    topics: BTreeMap<String, DocsManifest>,
}

#[derive(Debug, Clone)]
pub struct DocsManifest {
    pub title: String,
    pub description: Option<String>,
    pub children: Vec<String>,
    pub keywords: Vec<String>,
}

impl DocsIndex {
    pub fn new() -> Self {
        Self { topics: BTreeMap::new() }
    }

    pub fn add_topic(&mut self, name: &str, manifest: Option<Value>) {
        let manifest = manifest
            .map(DocsManifest::from_value)
            .unwrap_or_else(|| DocsManifest::default_for(name));
        self.topics.insert(name.to_string(), manifest);
    }

    pub fn remove_topic(&mut self, name: &str) {
        self.topics.remove(name);
    }

    /// List all topic names
    pub fn list_topics(&self) -> Value {
        let topics: Vec<Value> = self.topics.keys()
            .map(|k| Value::String(k.clone()))
            .collect();
        Value::Array(topics)
    }

    /// List topics with full metadata
    pub fn list_topics_full(&self) -> Value {
        let topics: Vec<Value> = self.topics.iter()
            .map(|(name, manifest)| {
                let mut map = BTreeMap::new();
                map.insert("name".into(), Value::String(name.clone()));
                map.insert("title".into(), Value::String(manifest.title.clone()));
                if let Some(ref desc) = manifest.description {
                    map.insert("description".into(), Value::String(desc.clone()));
                }
                Value::Map(map)
            })
            .collect();
        Value::Array(topics)
    }

    /// Search across all topics
    pub fn search(&self, query: &str) -> Value {
        let query_lower = query.to_lowercase();
        let matches: Vec<Value> = self.topics.iter()
            .filter(|(name, manifest)| {
                name.to_lowercase().contains(&query_lower)
                    || manifest.title.to_lowercase().contains(&query_lower)
                    || manifest.description.as_ref()
                        .map(|d| d.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
                    || manifest.keywords.iter()
                        .any(|k| k.to_lowercase().contains(&query_lower))
            })
            .map(|(name, manifest)| {
                let mut map = BTreeMap::new();
                map.insert("topic".into(), Value::String(name.clone()));
                map.insert("title".into(), Value::String(manifest.title.clone()));
                map.insert("path".into(), Value::String(format!("/ctx/help/{}", name)));
                Value::Map(map)
            })
            .collect();

        let mut result = BTreeMap::new();
        result.insert("query".into(), Value::String(query.to_string()));
        result.insert("count".into(), Value::Integer(matches.len() as i64));
        result.insert("results".into(), Value::Array(matches));
        Value::Map(result)
    }
}
```

### 3. Meta Endpoint

Expose redirect metadata for introspection:

```rust
impl HelpStore {
    fn read_meta(&self, path: &Path) -> Result<Option<Record>, Error> {
        if path.is_empty() {
            // GET /ctx/help/meta → all redirects
            // This requires access to OverlayStore's redirect table
            // Solution: HelpStore caches redirect info when notified
            return Ok(Some(Record::parsed(self.list_all_redirects())));
        }

        // GET /ctx/help/meta/{topic} → single redirect
        let topic = &path[0];
        Ok(self.get_redirect_info(topic).map(Record::parsed))
    }

    fn list_all_redirects(&self) -> Value {
        let redirects: Vec<Value> = self.redirects.iter()
            .map(|(from, info)| {
                let mut map = BTreeMap::new();
                map.insert("from".into(), Value::String(from.clone()));
                map.insert("to".into(), Value::String(info.target.clone()));
                map.insert("mode".into(), Value::String(format!("{:?}", info.mode)));
                Value::Map(map)
            })
            .collect();
        Value::Array(redirects)
    }
}
```

### 4. ReplDocsStore

Move all REPL documentation to a proper store:

```rust
// packages/repl/src/repl_docs_store.rs

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};
use std::collections::BTreeMap;

/// Documentation for the REPL itself.
///
/// Mounted at /ctx/repl, with docs at /ctx/repl/docs.
/// Discovery creates redirect: /ctx/help/repl → /ctx/repl/docs
pub struct ReplDocsStore {
    docs: BTreeMap<String, Value>,
}

impl ReplDocsStore {
    pub fn new() -> Self {
        let mut docs = BTreeMap::new();

        // Root manifest
        docs.insert(String::new(), Self::root_manifest());

        // Individual topics
        docs.insert("commands".into(), Self::commands_docs());
        docs.insert("registers".into(), Self::registers_docs());
        docs.insert("paths".into(), Self::paths_docs());
        docs.insert("examples".into(), Self::examples_docs());
        docs.insert("mounts".into(), Self::mounts_docs());

        Self { docs }
    }

    fn root_manifest() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("REPL Documentation".into()));
        map.insert("description".into(), Value::String(
            "Interactive command-line interface for StructFS".into()
        ));
        map.insert("children".into(), Value::Array(vec![
            Value::String("commands".into()),
            Value::String("registers".into()),
            Value::String("paths".into()),
            Value::String("mounts".into()),
            Value::String("examples".into()),
        ]));
        map.insert("keywords".into(), Value::Array(vec![
            Value::String("repl".into()),
            Value::String("cli".into()),
            Value::String("terminal".into()),
            Value::String("interactive".into()),
        ]));
        Value::Map(map)
    }

    fn commands_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Commands".into()));
        map.insert("description".into(), Value::String(
            "Available REPL commands and their syntax".into()
        ));

        let commands = vec![
            ("read", "read <path>", "Read value at path"),
            ("write", "write <path> <json>", "Write JSON value to path"),
            ("ls", "ls [path]", "List children at path"),
            ("cd", "cd <path>", "Change current directory"),
            ("pwd", "pwd", "Print current directory"),
            ("mounts", "mounts", "List all mount points"),
            ("registers", "registers", "List all registers"),
            ("help", "help [topic]", "Show help"),
        ];

        let command_list: Vec<Value> = commands.iter()
            .map(|(name, syntax, desc)| {
                let mut cmd = BTreeMap::new();
                cmd.insert("name".into(), Value::String(name.to_string()));
                cmd.insert("syntax".into(), Value::String(syntax.to_string()));
                cmd.insert("description".into(), Value::String(desc.to_string()));
                Value::Map(cmd)
            })
            .collect();

        map.insert("commands".into(), Value::Array(command_list));
        map.insert("aliases".into(), Self::command_aliases());
        Value::Map(map)
    }

    fn command_aliases() -> Value {
        let mut aliases = BTreeMap::new();
        aliases.insert("r".into(), Value::String("read".into()));
        aliases.insert("get".into(), Value::String("read".into()));
        aliases.insert("w".into(), Value::String("write".into()));
        aliases.insert("set".into(), Value::String("write".into()));
        aliases.insert("regs".into(), Value::String("registers".into()));
        Value::Map(aliases)
    }

    fn registers_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Registers".into()));
        map.insert("description".into(), Value::String(
            "Named storage for command outputs".into()
        ));

        let mut syntax = BTreeMap::new();
        syntax.insert("capture".into(), Value::String(
            "@name <command> - Store command output in register".into()
        ));
        syntax.insert("read".into(), Value::String(
            "read @name - Read register value".into()
        ));
        syntax.insert("dereference".into(), Value::String(
            "*@name - Use register value as path".into()
        ));
        syntax.insert("write".into(), Value::String(
            "write @name <value> - Set register directly".into()
        ));

        map.insert("syntax".into(), Value::Map(syntax));

        let examples = vec![
            "@result read /ctx/sys/time/now",
            "read @result",
            "@path read /ctx/sys/env/HOME",
            "read *@path",
        ];
        map.insert("examples".into(), Value::Array(
            examples.iter().map(|s| Value::String(s.to_string())).collect()
        ));

        Value::Map(map)
    }

    fn paths_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Path Syntax".into()));
        map.insert("description".into(), Value::String(
            "How paths work in StructFS".into()
        ));

        let mut rules = Vec::new();
        rules.push(Value::String("Paths are slash-separated components".into()));
        rules.push(Value::String("Leading slash is optional".into()));
        rules.push(Value::String("Components must be valid identifiers or integers".into()));
        rules.push(Value::String("Trailing slashes are normalized away".into()));
        rules.push(Value::String("Empty components (//) are normalized".into()));

        map.insert("rules".into(), Value::Array(rules));

        let examples = vec![
            ("/ctx/sys/time/now", "Absolute path"),
            ("ctx/sys/time/now", "Same path without leading slash"),
            ("data/users/0", "Numeric component for array access"),
        ];
        let example_list: Vec<Value> = examples.iter()
            .map(|(path, desc)| {
                let mut ex = BTreeMap::new();
                ex.insert("path".into(), Value::String(path.to_string()));
                ex.insert("description".into(), Value::String(desc.to_string()));
                Value::Map(ex)
            })
            .collect();

        map.insert("examples".into(), Value::Array(example_list));
        Value::Map(map)
    }

    fn mounts_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Mount System".into()));
        map.insert("description".into(), Value::String(
            "How stores are mounted and managed".into()
        ));

        let mut operations = BTreeMap::new();
        operations.insert("list".into(), Value::String(
            "read /ctx/mounts - List all mounts".into()
        ));
        operations.insert("mount".into(), Value::String(
            "write /ctx/mounts/<name> {\"type\": \"memory\"} - Create mount".into()
        ));
        operations.insert("unmount".into(), Value::String(
            "write /ctx/mounts/<name> null - Remove mount".into()
        ));
        operations.insert("inspect".into(), Value::String(
            "read /ctx/mounts/<name> - Get mount config".into()
        ));

        map.insert("operations".into(), Value::Map(operations));

        let mount_types = vec![
            ("memory", "In-memory JSON store"),
            ("local", "Local filesystem directory"),
            ("http", "HTTP client to base URL"),
            ("httpbroker", "Sync HTTP request broker"),
            ("asynchttpbroker", "Async HTTP request broker"),
        ];
        let type_list: Vec<Value> = mount_types.iter()
            .map(|(name, desc)| {
                let mut t = BTreeMap::new();
                t.insert("type".into(), Value::String(name.to_string()));
                t.insert("description".into(), Value::String(desc.to_string()));
                Value::Map(t)
            })
            .collect();

        map.insert("types".into(), Value::Array(type_list));
        Value::Map(map)
    }

    fn examples_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Examples".into()));
        map.insert("description".into(), Value::String(
            "Common usage patterns".into()
        ));

        let examples = vec![
            ("Read system time", vec![
                "read /ctx/sys/time/now",
            ]),
            ("Make HTTP request", vec![
                "@req write /ctx/http {\"method\": \"GET\", \"path\": \"https://api.example.com/data\"}",
                "read *@req",
            ]),
            ("Create and use a store", vec![
                "write /ctx/mounts/mydata {\"type\": \"memory\"}",
                "write /mydata/users/alice {\"name\": \"Alice\", \"age\": 30}",
                "read /mydata/users/alice",
            ]),
            ("Work with registers", vec![
                "@home read /ctx/sys/env/HOME",
                "read @home",
            ]),
        ];

        let example_list: Vec<Value> = examples.iter()
            .map(|(title, commands)| {
                let mut ex = BTreeMap::new();
                ex.insert("title".into(), Value::String(title.to_string()));
                ex.insert("commands".into(), Value::Array(
                    commands.iter().map(|c| Value::String(c.to_string())).collect()
                ));
                Value::Map(ex)
            })
            .collect();

        map.insert("examples".into(), Value::Array(example_list));
        Value::Map(map)
    }
}

impl Reader for ReplDocsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Must be under /docs path
        if from.is_empty() {
            return Ok(None); // Root of ReplStore, not docs
        }

        if from[0] != "docs" {
            return Ok(None); // Not a docs path
        }

        // Strip "docs" prefix
        let doc_path = if from.len() > 1 {
            from.slice(1, from.len()).to_string()
        } else {
            String::new()
        };

        Ok(self.docs.get(&doc_path).cloned().map(Record::parsed))
    }
}

impl Writer for ReplDocsStore {
    fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::store("repl_docs", "write", "REPL docs are read-only"))
    }
}

impl Default for ReplDocsStore {
    fn default() -> Self {
        Self::new()
    }
}
```

### 5. Notification System

HelpStore needs to be notified when redirects are created/removed:

```rust
// In MountStore or OverlayStore

/// Callback for docs discovery events
pub trait DocsObserver {
    fn on_docs_discovered(&mut self, topic: &str, manifest: Option<Value>);
    fn on_docs_removed(&mut self, topic: &str);
}

impl<F: StoreFactory> MountStore<F> {
    pub fn set_docs_observer(&mut self, observer: Weak<RefCell<dyn DocsObserver>>) {
        self.docs_observer = Some(observer);
    }

    fn discover_and_redirect_docs(&mut self, name: &str, mount_path: &Path) -> Result<(), Error> {
        let docs_path = mount_path.join(&path!("docs"));

        if let Ok(Some(record)) = self.overlay.read(&docs_path) {
            // Create redirect
            let help_path = path!("ctx/help").join(&Path::parse(name)?);
            self.overlay.add_redirect(
                help_path,
                docs_path.clone(),
                RedirectMode::ReadOnly,
                Some(name.to_string()),
            );

            // Notify observer (HelpStore)
            if let Some(ref observer) = self.docs_observer {
                if let Some(obs) = observer.upgrade() {
                    let manifest = record.into_value(&NoCodec).ok();
                    obs.borrow_mut().on_docs_discovered(name, manifest);
                }
            }
        }

        Ok(())
    }
}
```

**Alternative: Simpler approach without callbacks**

HelpStore queries the redirect table directly:

```rust
impl HelpStore {
    /// Set reference to overlay for redirect introspection
    pub fn set_overlay(&mut self, overlay: Weak<RefCell<OverlayStore>>) {
        self.overlay_ref = Some(overlay);
    }

    fn list_topics(&self) -> Value {
        if let Some(ref overlay) = self.overlay_ref {
            if let Some(overlay) = overlay.upgrade() {
                let topics: Vec<Value> = overlay.borrow()
                    .list_redirects()
                    .iter()
                    .filter(|(from, _, _)| from.has_prefix(&path!("ctx/help")))
                    .filter_map(|(from, _, _)| {
                        // Extract topic name from /ctx/help/{topic}
                        if from.len() > 2 {
                            Some(Value::String(from[2].clone()))
                        } else {
                            None
                        }
                    })
                    .collect();
                return Value::Array(topics);
            }
        }
        Value::Array(vec![])
    }
}
```

### 6. Store Initialization Order

```rust
impl StoreContext {
    pub fn new() -> Result<Self, ContextError> {
        let mut store = MountStore::new(CoreReplStoreFactory);

        // 1. Mount HelpStore first (it's the aggregator)
        let help_store = HelpStore::new();
        store.mount_store("ctx/help", Box::new(help_store))?;

        // 2. Mount ReplDocsStore (REPL's own documentation)
        store.mount_store("ctx/repl", Box::new(ReplDocsStore::new()))?;

        // 3. Mount other stores - discovery creates help redirects
        store.mount_store("ctx/sys", Box::new(SysStore::new()))?;
        store.mount_store("ctx/http_sync", Box::new(HttpBrokerStore::new()))?;
        store.mount_store("ctx/http", Box::new(AsyncHttpBrokerStore::new()))?;

        // 4. Mount RegisterStore
        store.mount_store("ctx/registers", Box::new(RegisterStore::new()))?;

        Ok(Self {
            store,
            current_path: Path::root(),
        })
    }
}
```

After initialization:
- `/ctx/help` → HelpStore (lists topics, search, meta)
- `/ctx/help/repl` → REDIRECT to `/ctx/repl/docs`
- `/ctx/help/sys` → REDIRECT to `/ctx/sys/docs`
- `/ctx/help/http` → REDIRECT to `/ctx/http/docs`
- `/ctx/help/http_sync` → REDIRECT to `/ctx/http_sync/docs`

## Path Reference

### HelpStore Paths (Direct)

| Path | Method | Returns |
|------|--------|---------|
| `/ctx/help` | read | `["repl", "sys", "http", "http_sync"]` |
| `/ctx/help/meta` | read | `[{from: "/ctx/help/sys", to: "/ctx/sys/docs", mode: "ReadOnly"}, ...]` |
| `/ctx/help/meta/{topic}` | read | `{from: "...", to: "...", mode: "..."}` |
| `/ctx/help/search/{query}` | read | `{query: "...", count: N, results: [...]}` |

### HelpStore Paths (Redirected)

| Path | Redirects To |
|------|--------------|
| `/ctx/help/repl` | `/ctx/repl/docs` |
| `/ctx/help/repl/commands` | `/ctx/repl/docs/commands` |
| `/ctx/help/sys` | `/ctx/sys/docs` |
| `/ctx/help/sys/env` | `/ctx/sys/docs/env` |
| `/ctx/help/http` | `/ctx/http/docs` |

### ReplDocsStore Paths

| Path | Returns |
|------|---------|
| `/ctx/repl/docs` | Root manifest: title, description, children |
| `/ctx/repl/docs/commands` | Command reference |
| `/ctx/repl/docs/registers` | Register syntax |
| `/ctx/repl/docs/paths` | Path syntax |
| `/ctx/repl/docs/mounts` | Mount system |
| `/ctx/repl/docs/examples` | Usage examples |

## Implementation Steps

### Phase 1: Create ReplDocsStore (Day 1)

1. Create `packages/repl/src/repl_docs_store.rs`
2. Implement all documentation methods
3. Add to mod.rs exports
4. Write tests

### Phase 2: Simplify HelpStore (Day 1)

1. Remove all hard-coded topic methods from HelpStore
2. Add DocsIndex struct
3. Implement `list_topics()` from redirect table
4. Implement `read_meta()`
5. Implement `read_search()`
6. Update tests

### Phase 3: Wire Notifications (Day 2)

1. Add docs observer callback to MountStore (or simpler: overlay reference)
2. Update `discover_and_redirect_docs()` to notify HelpStore
3. Update `unmount()` cascade to notify HelpStore
4. Test notification flow

### Phase 4: Update Store Initialization (Day 2)

1. Update StoreContext to mount ReplDocsStore
2. Ensure mount order is correct
3. Verify all redirects created
4. Integration tests

### Phase 5: Delete Dead Code (Day 2)

1. Remove old help topic methods from HelpStore
2. Remove any remaining hard-coded documentation
3. Verify tests still pass
4. Update any documentation

## Test Cases

### HelpStore Tests

```rust
#[test]
fn help_lists_all_topics() {
    let ctx = StoreContext::new().unwrap();
    let result = ctx.store.read(&path!("ctx/help")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    match value {
        Value::Array(topics) => {
            assert!(topics.contains(&Value::String("repl".into())));
            assert!(topics.contains(&Value::String("sys".into())));
            assert!(topics.contains(&Value::String("http".into())));
        }
        _ => panic!("Expected array"),
    }
}

#[test]
fn help_meta_shows_redirects() {
    let ctx = StoreContext::new().unwrap();
    let result = ctx.store.read(&path!("ctx/help/meta")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    match value {
        Value::Array(redirects) => {
            assert!(!redirects.is_empty());
            // Check structure of first redirect
            if let Value::Map(redirect) = &redirects[0] {
                assert!(redirect.contains_key("from"));
                assert!(redirect.contains_key("to"));
                assert!(redirect.contains_key("mode"));
            }
        }
        _ => panic!("Expected array"),
    }
}

#[test]
fn help_search_finds_topics() {
    let ctx = StoreContext::new().unwrap();
    let result = ctx.store.read(&path!("ctx/help/search/time")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    match value {
        Value::Map(result) => {
            assert_eq!(result.get("query"), Some(&Value::String("time".into())));
            if let Some(Value::Array(results)) = result.get("results") {
                assert!(!results.is_empty());
            }
        }
        _ => panic!("Expected map"),
    }
}

#[test]
fn help_redirects_to_store_docs() {
    let ctx = StoreContext::new().unwrap();

    // Reading /ctx/help/sys should redirect to /ctx/sys/docs
    let result = ctx.store.read(&path!("ctx/help/sys")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    // Should have the sys docs manifest
    match value {
        Value::Map(map) => {
            assert!(map.contains_key("title"));
            assert!(map.contains_key("children"));
        }
        _ => panic!("Expected sys docs manifest"),
    }
}
```

### ReplDocsStore Tests

```rust
#[test]
fn repl_docs_has_root_manifest() {
    let mut store = ReplDocsStore::new();
    let result = store.read(&path!("docs")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    match value {
        Value::Map(map) => {
            assert_eq!(map.get("title"), Some(&Value::String("REPL Documentation".into())));
            assert!(map.contains_key("children"));
        }
        _ => panic!("Expected map"),
    }
}

#[test]
fn repl_docs_has_commands() {
    let mut store = ReplDocsStore::new();
    let result = store.read(&path!("docs/commands")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    match value {
        Value::Map(map) => {
            assert!(map.contains_key("commands"));
        }
        _ => panic!("Expected map"),
    }
}

#[test]
fn repl_docs_is_read_only() {
    let mut store = ReplDocsStore::new();
    let result = store.write(&path!("docs/test"), Record::parsed(Value::Null));
    assert!(result.is_err());
}
```

### Integration Tests

```rust
#[test]
fn unmount_removes_help_redirect() {
    let mut ctx = StoreContext::new().unwrap();

    // Mount a store with docs
    ctx.store.mount("test", MountConfig::Memory).unwrap();
    // (Assuming memory store has docs for this test)

    // Verify redirect exists
    let topics = get_help_topics(&mut ctx);
    let had_test = topics.contains(&"test".to_string());

    // Unmount
    ctx.store.unmount("test").unwrap();

    // Verify redirect removed
    let topics = get_help_topics(&mut ctx);
    assert!(!topics.contains(&"test".to_string()));
}

#[test]
fn help_repl_redirects_correctly() {
    let ctx = StoreContext::new().unwrap();

    // /ctx/help/repl should give same content as /ctx/repl/docs
    let help_result = ctx.store.read(&path!("ctx/help/repl")).unwrap();
    let direct_result = ctx.store.read(&path!("ctx/repl/docs")).unwrap();

    // Both should return the same manifest
    assert_eq!(help_result, direct_result);
}
```

## Files Changed

### New Files
- `packages/repl/src/repl_docs_store.rs` - REPL documentation store
- `packages/repl/src/docs_index.rs` - Search index (optional, can be in help_store.rs)

### Modified Files
- `packages/repl/src/help_store.rs` - Gut and simplify
- `packages/repl/src/store_context.rs` - Mount ReplDocsStore
- `packages/repl/src/mod.rs` - Export new modules
- `packages/core-store/src/mount_store.rs` - Add observer callback (if needed)

### Deleted Code
- All `fn *_help(&self) -> Value` methods in HelpStore (~400 lines)

## Success Criteria

1. **`read /ctx/help`** returns dynamic list from redirect table
2. **`read /ctx/help/meta`** exposes all redirects
3. **`read /ctx/help/search/time`** finds relevant topics
4. **`read /ctx/help/repl`** redirects to `/ctx/repl/docs`
5. **Unmounting a store** removes its help redirect
6. **HelpStore has no hard-coded content** - pure aggregator
7. **All tests pass** with >90% coverage on new code

## The S-Tier Difference

| Aspect | Current (C-Tier) | S-Tier |
|--------|------------------|--------|
| Topic listing | None | Dynamic from redirects |
| Search | None | Full-text across all docs |
| Introspection | None | `/meta` shows all redirects |
| REPL docs | Hard-coded in HelpStore | ReplDocsStore |
| Consistency | REPL is special case | Everything is a store |
| Discovery | User must know paths | `read /ctx/help` tells you |

This is what "everything is a store" looks like when you actually mean it.
