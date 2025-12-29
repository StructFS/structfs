//! Help store that provides documentation aggregation, search, and metadata.
//!
//! Mount at `/ctx/help` to provide:
//! - `read /ctx/help` - List all available topics
//! - `read /ctx/help/meta` - All redirect mappings
//! - `read /ctx/help/meta/{topic}` - Single redirect info
//! - `read /ctx/help/search/{query}` - Search across all topics
//!
//! ## Docs Protocol
//!
//! Store-specific documentation is provided by stores themselves via a `docs` path.
//! When a store is mounted, the routing system automatically creates a redirect
//! from `/ctx/help/{name}` to `{mount}/docs` if the store provides docs.
//!
//! For example, mounting SysStore at `/ctx/sys` with docs at `/ctx/sys/docs`
//! creates a redirect: `/ctx/help/sys` -> `/ctx/sys/docs`.
//!
//! HelpStore holds NO content itself - it is purely an aggregator.

use collection_literals::btree;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use structfs_core_store::overlay_store::RedirectMode;
use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// Manifest for a docs topic (used for search indexing).
#[derive(Debug, Clone)]
pub struct DocsManifest {
    pub title: String,
    pub description: Option<String>,
    pub children: Vec<String>,
    pub keywords: Vec<String>,
}

impl DocsManifest {
    /// Create a default manifest for a topic.
    pub fn default_for(name: &str) -> Self {
        Self {
            title: name.to_string(),
            description: None,
            children: Vec::new(),
            keywords: Vec::new(),
        }
    }

    /// Parse a manifest from a Value.
    pub fn from_value(value: Value) -> Self {
        match value {
            Value::Map(map) => {
                let title = map
                    .get("title")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();

                let description = map.get("description").and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });

                let children = map
                    .get("children")
                    .and_then(|v| match v {
                        Value::Array(arr) => Some(
                            arr.iter()
                                .filter_map(|v| match v {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .collect(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_default();

                let keywords = map
                    .get("keywords")
                    .and_then(|v| match v {
                        Value::Array(arr) => Some(
                            arr.iter()
                                .filter_map(|v| match v {
                                    Value::String(s) => Some(s.clone()),
                                    _ => None,
                                })
                                .collect(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_default();

                Self {
                    title,
                    description,
                    children,
                    keywords,
                }
            }
            _ => Self::default_for("unknown"),
        }
    }
}

/// Index for topic listing and search.
#[derive(Debug, Clone, Default)]
pub struct DocsIndex {
    /// topic_name -> DocsManifest
    topics: BTreeMap<String, DocsManifest>,
}

impl DocsIndex {
    pub fn new() -> Self {
        Self {
            topics: BTreeMap::new(),
        }
    }

    /// Add a topic to the index.
    pub fn add_topic(&mut self, name: &str, manifest: Option<Value>) {
        let manifest = manifest
            .map(DocsManifest::from_value)
            .unwrap_or_else(|| DocsManifest::default_for(name));
        self.topics.insert(name.to_string(), manifest);
    }

    /// Remove a topic from the index.
    pub fn remove_topic(&mut self, name: &str) {
        self.topics.remove(name);
    }

    /// List all topic names.
    pub fn list_topics(&self) -> Value {
        let topics: Vec<Value> = self
            .topics
            .keys()
            .map(|k| Value::String(k.clone()))
            .collect();
        Value::Array(topics)
    }

    /// List topics with full metadata.
    pub fn list_topics_full(&self) -> Value {
        let topics: Vec<Value> = self
            .topics
            .iter()
            .map(|(name, manifest)| {
                let mut map = btree! {
                    "name".into() => Value::String(name.clone()),
                    "title".into() => Value::String(manifest.title.clone()),
                };
                if let Some(ref desc) = manifest.description {
                    map.insert("description".into(), Value::String(desc.clone()));
                }
                Value::Map(map)
            })
            .collect();
        Value::Array(topics)
    }

    /// Search across all topics.
    pub fn search(&self, query: &str) -> Value {
        let query_lower = query.to_lowercase();
        let matches: Vec<Value> = self
            .topics
            .iter()
            .filter(|(name, manifest)| {
                name.to_lowercase().contains(&query_lower)
                    || manifest.title.to_lowercase().contains(&query_lower)
                    || manifest
                        .description
                        .as_ref()
                        .is_some_and(|d| d.to_lowercase().contains(&query_lower))
                    || manifest
                        .keywords
                        .iter()
                        .any(|k| k.to_lowercase().contains(&query_lower))
            })
            .map(|(name, manifest)| {
                Value::Map(btree! {
                    "topic".into() => Value::String(name.clone()),
                    "title".into() => Value::String(manifest.title.clone()),
                    "path".into() => Value::String(format!("/ctx/help/{}", name)),
                })
            })
            .collect();

        Value::Map(btree! {
            "query".into() => Value::String(query.to_string()),
            "count".into() => Value::Integer(matches.len() as i64),
            "results".into() => Value::Array(matches),
        })
    }
}

/// Redirect information stored for introspection.
#[derive(Debug, Clone)]
pub struct RedirectInfo {
    pub from: String,
    pub to: String,
    pub mode: RedirectMode,
}

/// Shared state for HelpStore, allowing updates after mounting.
#[derive(Debug, Default)]
pub struct HelpStoreState {
    /// Index for search functionality
    pub index: DocsIndex,
    /// Cached redirect info for introspection
    pub redirects: BTreeMap<String, RedirectInfo>,
}

impl HelpStoreState {
    pub fn new() -> Self {
        Self {
            index: DocsIndex::new(),
            redirects: BTreeMap::new(),
        }
    }

    /// Called when a docs redirect is created.
    pub fn index_docs(&mut self, topic: &str, manifest: Option<Value>) {
        self.index.add_topic(topic, manifest);
    }

    /// Called when a docs redirect is removed.
    pub fn unindex_docs(&mut self, topic: &str) {
        self.index.remove_topic(topic);
        self.redirects.remove(topic);
    }

    /// Register a redirect for introspection.
    pub fn register_redirect(&mut self, topic: &str, from: &str, to: &str, mode: RedirectMode) {
        self.redirects.insert(
            topic.to_string(),
            RedirectInfo {
                from: from.to_string(),
                to: to.to_string(),
                mode,
            },
        );
    }
}

/// Handle to shared HelpStore state for external updates.
pub type HelpStoreHandle = Arc<RwLock<HelpStoreState>>;

/// A store that provides help documentation aggregation.
///
/// The HelpStore provides three services:
/// 1. Topic listing - Derived from indexed topics
/// 2. Metadata - Expose redirect mappings
/// 3. Search - Query across all indexed docs
///
/// Store-specific docs are accessed via redirects (handled by OverlayStore).
///
/// The internal state is wrapped in `Arc<RwLock<>>` so it can be updated
/// after mounting (e.g., when stores are dynamically mounted/unmounted).
pub struct HelpStore {
    state: HelpStoreHandle,
}

impl HelpStore {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(HelpStoreState::new())),
        }
    }

    /// Create a HelpStore with shared state.
    ///
    /// Use this when you need to update the help index after mounting.
    pub fn with_shared_state(state: HelpStoreHandle) -> Self {
        Self { state }
    }

    /// Get a handle to the shared state for external updates.
    pub fn handle(&self) -> HelpStoreHandle {
        Arc::clone(&self.state)
    }

    fn read_meta(&self, path: &Path) -> Result<Option<Record>, Error> {
        let state = self
            .state
            .read()
            .map_err(|_| Error::store("help", "read", "Failed to acquire read lock"))?;

        if path.is_empty() {
            // GET /ctx/help/meta -> all redirects
            return Ok(Some(Record::parsed(Self::list_all_redirects(&state))));
        }

        // GET /ctx/help/meta/{topic} -> single redirect
        let topic = &path[0];
        Ok(Self::get_redirect_info(&state, topic).map(Record::parsed))
    }

    fn list_all_redirects(state: &HelpStoreState) -> Value {
        let redirects: Vec<Value> = state
            .redirects
            .iter()
            .map(|(topic, info)| {
                Value::Map(btree! {
                    "topic".into() => Value::String(topic.clone()),
                    "from".into() => Value::String(info.from.clone()),
                    "to".into() => Value::String(info.to.clone()),
                    "mode".into() => Value::String(format!("{:?}", info.mode)),
                })
            })
            .collect();
        Value::Array(redirects)
    }

    fn get_redirect_info(state: &HelpStoreState, topic: &str) -> Option<Value> {
        state.redirects.get(topic).map(|info| {
            Value::Map(btree! {
                "topic".into() => Value::String(topic.to_string()),
                "from".into() => Value::String(info.from.clone()),
                "to".into() => Value::String(info.to.clone()),
                "mode".into() => Value::String(format!("{:?}", info.mode)),
            })
        })
    }

    fn read_search(&self, path: &Path) -> Result<Option<Record>, Error> {
        let state = self
            .state
            .read()
            .map_err(|_| Error::store("help", "read", "Failed to acquire read lock"))?;

        if path.is_empty() {
            // No query provided
            return Ok(Some(Record::parsed(Value::Map(btree! {
                "error".into() => Value::String("No search query provided".into()),
                "usage".into() => Value::String("read /ctx/help/search/<query>".into()),
            }))));
        }

        // The query is the full remaining path (allows multi-word queries)
        let query = path.components.join("/");
        Ok(Some(Record::parsed(state.index.search(&query))))
    }
}

impl Default for HelpStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for HelpStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            // GET /ctx/help -> list all topics
            let state = self
                .state
                .read()
                .map_err(|_| Error::store("help", "read", "Failed to acquire read lock"))?;
            return Ok(Some(Record::parsed(state.index.list_topics())));
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

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, NoCodec};

    fn read_help(help: &mut HelpStore, path: &str) -> Value {
        let result = help.read(&Path::parse(path).unwrap()).unwrap();
        result.unwrap().into_value(&NoCodec).unwrap()
    }

    #[test]
    fn test_help_root_empty() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "");
        match value {
            Value::Array(arr) => {
                assert!(arr.is_empty());
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_help_root_with_topics() {
        let state = Arc::new(RwLock::new(HelpStoreState::new()));
        state.write().unwrap().index_docs("sys", None);
        state.write().unwrap().index_docs("repl", None);

        let mut help = HelpStore::with_shared_state(state);
        let value = read_help(&mut help, "");
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
                assert!(arr.contains(&Value::String("sys".into())));
                assert!(arr.contains(&Value::String("repl".into())));
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_help_meta_empty() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "meta");
        match value {
            Value::Array(arr) => {
                assert!(arr.is_empty());
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_help_meta_with_redirects() {
        let state = Arc::new(RwLock::new(HelpStoreState::new()));
        state.write().unwrap().register_redirect(
            "sys",
            "/ctx/help/sys",
            "/ctx/sys/docs",
            RedirectMode::ReadOnly,
        );

        let mut help = HelpStore::with_shared_state(state);
        let value = read_help(&mut help, "meta");
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 1);
                if let Value::Map(map) = &arr[0] {
                    assert_eq!(map.get("topic"), Some(&Value::String("sys".into())));
                    assert_eq!(map.get("to"), Some(&Value::String("/ctx/sys/docs".into())));
                }
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_help_meta_single() {
        let state = Arc::new(RwLock::new(HelpStoreState::new()));
        state.write().unwrap().register_redirect(
            "sys",
            "/ctx/help/sys",
            "/ctx/sys/docs",
            RedirectMode::ReadOnly,
        );

        let mut help = HelpStore::with_shared_state(state);
        let value = read_help(&mut help, "meta/sys");
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("topic"), Some(&Value::String("sys".into())));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_meta_single_not_found() {
        let mut help = HelpStore::new();
        let result = help.read(&path!("meta/nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_help_search_no_query() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "search");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("error"));
                assert!(map.contains_key("usage"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_search_finds_topics() {
        let state = Arc::new(RwLock::new(HelpStoreState::new()));
        state.write().unwrap().index_docs(
            "sys",
            Some(Value::Map({
                let mut m = BTreeMap::new();
                m.insert("title".into(), Value::String("System Primitives".into()));
                m.insert(
                    "keywords".into(),
                    Value::Array(vec![Value::String("time".into())]),
                );
                m
            })),
        );

        let mut help = HelpStore::with_shared_state(state);
        let value = read_help(&mut help, "search/time");
        match value {
            Value::Map(result) => {
                assert_eq!(result.get("query"), Some(&Value::String("time".into())));
                assert_eq!(result.get("count"), Some(&Value::Integer(1)));
                if let Some(Value::Array(results)) = result.get("results") {
                    assert_eq!(results.len(), 1);
                }
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_search_no_results() {
        let state = Arc::new(RwLock::new(HelpStoreState::new()));
        state.write().unwrap().index_docs("sys", None);

        let mut help = HelpStore::with_shared_state(state);
        let value = read_help(&mut help, "search/nonexistent");
        match value {
            Value::Map(result) => {
                assert_eq!(result.get("count"), Some(&Value::Integer(0)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_unknown_topic_returns_none() {
        let mut help = HelpStore::new();
        let result = help.read(&path!("unknown")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_help_read_only() {
        let mut help = HelpStore::new();
        let result = help.write(&path!("test"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn test_default_impl() {
        let _help: HelpStore = Default::default();
    }

    #[test]
    fn test_index_and_unindex() {
        let state = Arc::new(RwLock::new(HelpStoreState::new()));
        state.write().unwrap().index_docs("test", None);
        state.write().unwrap().register_redirect(
            "test",
            "/ctx/help/test",
            "/test/docs",
            RedirectMode::ReadOnly,
        );

        let mut help = HelpStore::with_shared_state(Arc::clone(&state));

        // Should have topic and redirect
        let topics = read_help(&mut help, "");
        match topics {
            Value::Array(arr) => assert_eq!(arr.len(), 1),
            _ => panic!("Expected array"),
        }

        let meta = read_help(&mut help, "meta");
        match meta {
            Value::Array(arr) => assert_eq!(arr.len(), 1),
            _ => panic!("Expected array"),
        }

        // Unindex via shared state
        state.write().unwrap().unindex_docs("test");

        let topics = read_help(&mut help, "");
        match topics {
            Value::Array(arr) => assert!(arr.is_empty()),
            _ => panic!("Expected array"),
        }

        let meta = read_help(&mut help, "meta");
        match meta {
            Value::Array(arr) => assert!(arr.is_empty()),
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_docs_manifest_from_value() {
        let mut m = BTreeMap::new();
        m.insert("title".into(), Value::String("Test Title".into()));
        m.insert(
            "description".into(),
            Value::String("Test Description".into()),
        );
        m.insert(
            "children".into(),
            Value::Array(vec![Value::String("a".into()), Value::String("b".into())]),
        );
        m.insert(
            "keywords".into(),
            Value::Array(vec![Value::String("k1".into())]),
        );

        let manifest = DocsManifest::from_value(Value::Map(m));
        assert_eq!(manifest.title, "Test Title");
        assert_eq!(manifest.description, Some("Test Description".to_string()));
        assert_eq!(manifest.children, vec!["a", "b"]);
        assert_eq!(manifest.keywords, vec!["k1"]);
    }

    #[test]
    fn test_docs_manifest_default_for() {
        let manifest = DocsManifest::default_for("test");
        assert_eq!(manifest.title, "test");
        assert!(manifest.description.is_none());
        assert!(manifest.children.is_empty());
        assert!(manifest.keywords.is_empty());
    }

    #[test]
    fn test_docs_manifest_from_non_map() {
        let manifest = DocsManifest::from_value(Value::String("not a map".into()));
        assert_eq!(manifest.title, "unknown");
    }

    #[test]
    fn test_docs_index_list_topics_full() {
        let mut index = DocsIndex::new();
        index.add_topic(
            "test",
            Some(Value::Map({
                let mut m = BTreeMap::new();
                m.insert("title".into(), Value::String("Test Title".into()));
                m.insert("description".into(), Value::String("Test Desc".into()));
                m
            })),
        );

        let value = index.list_topics_full();
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 1);
                if let Value::Map(map) = &arr[0] {
                    assert_eq!(map.get("name"), Some(&Value::String("test".into())));
                    assert_eq!(map.get("title"), Some(&Value::String("Test Title".into())));
                    assert_eq!(
                        map.get("description"),
                        Some(&Value::String("Test Desc".into()))
                    );
                }
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_search_by_name() {
        let mut index = DocsIndex::new();
        index.add_topic("system", None);

        let result = index.search("sys");
        match result {
            Value::Map(m) => {
                assert_eq!(m.get("count"), Some(&Value::Integer(1)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_search_by_title() {
        let mut index = DocsIndex::new();
        index.add_topic(
            "sys",
            Some(Value::Map({
                let mut m = BTreeMap::new();
                m.insert("title".into(), Value::String("System Primitives".into()));
                m
            })),
        );

        let result = index.search("primitives");
        match result {
            Value::Map(m) => {
                assert_eq!(m.get("count"), Some(&Value::Integer(1)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_search_by_description() {
        let mut index = DocsIndex::new();
        index.add_topic(
            "sys",
            Some(Value::Map({
                let mut m = BTreeMap::new();
                m.insert(
                    "description".into(),
                    Value::String("OS primitives for time and env".into()),
                );
                m
            })),
        );

        let result = index.search("primitives");
        match result {
            Value::Map(m) => {
                assert_eq!(m.get("count"), Some(&Value::Integer(1)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_search_case_insensitive() {
        let mut index = DocsIndex::new();
        index.add_topic(
            "SYS",
            Some(Value::Map({
                let mut m = BTreeMap::new();
                m.insert("title".into(), Value::String("SYSTEM".into()));
                m
            })),
        );

        let result = index.search("sys");
        match result {
            Value::Map(m) => {
                assert_eq!(m.get("count"), Some(&Value::Integer(1)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_handle_returns_shared_state() {
        let help = HelpStore::new();
        let handle = help.handle();

        // Modify via handle
        handle.write().unwrap().index_docs("test", None);

        // Should be visible through HelpStore
        let mut help = HelpStore::with_shared_state(handle);
        let value = read_help(&mut help, "");
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 1);
            }
            _ => panic!("Expected array"),
        }
    }
}
