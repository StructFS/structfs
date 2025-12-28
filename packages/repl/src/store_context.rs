//! Store context for the REPL.
//!
//! This module provides the store context that manages mounts and registers.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use structfs_core_store::{
    mount_store::{MountConfig, MountStore, StoreFactory},
    overlay_store::StoreBox,
    Error as CoreError, NoCodec, Path, Reader, Record, Value, Writer,
};

use structfs_serde_store::{json_to_value, value_to_json};

// Import store implementations
use crate::help_store::{HelpStore, HelpStoreHandle, HelpStoreState};
use crate::repl_docs_store::ReplDocsStore;
use structfs_http::{AsyncHttpBrokerStore, HttpBrokerStore};
use structfs_json_store::InMemoryStore;
use structfs_sys::SysStore;

#[derive(thiserror::Error, Debug)]
pub enum ContextError {
    #[error("Store error: {0}")]
    Store(#[from] CoreError),

    #[error("HTTP error: {0}")]
    Http(#[from] structfs_http::Error),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Factory for creating stores from mount configurations.
///
/// This is the default factory used by StoreContext. It creates stores for
/// all standard mount configurations (memory, HTTP, sys, help, etc.).
pub struct CoreReplStoreFactory;

impl StoreFactory for CoreReplStoreFactory {
    fn create(&self, config: &MountConfig) -> Result<StoreBox, CoreError> {
        match config {
            MountConfig::Memory => Ok(Box::new(InMemoryStore::new())),
            MountConfig::Local { path: _ } => {
                // Local disk store not yet migrated to new architecture
                Err(CoreError::store(
                    "factory",
                    "create",
                    "Local disk store not yet available in new architecture",
                ))
            }
            MountConfig::Http { url: _ } => {
                // HTTP client not using direct mode in REPL context
                Err(CoreError::store(
                    "factory",
                    "create",
                    "HTTP client store not yet available in new architecture",
                ))
            }
            MountConfig::HttpBroker => {
                let store = HttpBrokerStore::with_default_timeout().map_err(|e| {
                    CoreError::store(
                        "factory",
                        "create",
                        format!("Failed to create HTTP broker: {}", e),
                    )
                })?;
                Ok(Box::new(store))
            }
            MountConfig::AsyncHttpBroker => {
                let store = AsyncHttpBrokerStore::with_default_timeout().map_err(|e| {
                    CoreError::store(
                        "factory",
                        "create",
                        format!("Failed to create async HTTP broker: {}", e),
                    )
                })?;
                Ok(Box::new(store))
            }
            MountConfig::Structfs { url: _ } => Err(CoreError::store(
                "factory",
                "create",
                "Remote StructFS not yet available in new architecture",
            )),
            MountConfig::Help => Ok(Box::new(HelpStore::new())),
            MountConfig::Sys => Ok(Box::new(SysStore::new())),
            MountConfig::Repl => Ok(Box::new(ReplDocsStore::new())),
            MountConfig::Registers => Ok(Box::new(RegisterStore::new())),
        }
    }
}

/// Register store using Value instead of JsonValue (new architecture)
pub struct RegisterStore {
    registers: BTreeMap<String, Value>,
}

impl RegisterStore {
    pub fn new() -> Self {
        Self {
            registers: BTreeMap::new(),
        }
    }

    /// Get a register value by name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.registers.get(name)
    }

    /// Set a register value.
    pub fn set(&mut self, name: &str, value: Value) {
        self.registers.insert(name.to_string(), value);
    }

    /// List all register names.
    pub fn list(&self) -> Vec<&String> {
        self.registers.keys().collect()
    }

    /// Navigate into a Value by path.
    fn navigate<'a>(value: &'a Value, path: &Path) -> Option<&'a Value> {
        let mut current = value;
        for component in path.iter() {
            current = match current {
                Value::Map(map) => map.get(component.as_str())?,
                Value::Array(arr) => {
                    let index: usize = component.parse().ok()?;
                    arr.get(index)?
                }
                _ => return None,
            };
        }
        Some(current)
    }
}

impl Default for RegisterStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for RegisterStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, CoreError> {
        if from.is_empty() {
            // List all registers
            let list: Vec<Value> = self
                .registers
                .keys()
                .map(|k| Value::String(k.clone()))
                .collect();
            return Ok(Some(Record::parsed(Value::Array(list))));
        }

        let register_name = &from[0];
        let sub_path = from.slice(1, from.len());

        let register_value = match self.registers.get(register_name.as_str()) {
            Some(v) => v,
            None => return Ok(None),
        };

        let value = if sub_path.is_empty() {
            register_value.clone()
        } else {
            match Self::navigate(register_value, &sub_path) {
                Some(v) => v.clone(),
                None => return Ok(None),
            }
        };

        Ok(Some(Record::parsed(value)))
    }
}

impl Writer for RegisterStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, CoreError> {
        if to.is_empty() {
            return Err(CoreError::store(
                "register",
                "write",
                "Cannot write to register root. Use @name to specify a register.",
            ));
        }

        let value = data.into_value(&NoCodec)?;
        let register_name = &to[0];

        // For simplicity, we only support writing to the register itself (not sub-paths)
        // Full sub-path support could be added later
        self.registers.insert(register_name.to_string(), value);
        Ok(to.clone())
    }
}

/// Store context using the new architecture.
///
/// The context is generic over a `StoreFactory` implementation, allowing
/// different factories to be used for testing or alternative configurations.
/// By default, it uses `CoreReplStoreFactory` which creates all standard stores.
///
/// Registers are now mounted at `/ctx/registers/` rather than embedded.
/// Use `@name` syntax as sugar for `/ctx/registers/name`.
pub struct StoreContext<F: StoreFactory = CoreReplStoreFactory> {
    store: MountStore<F>,
    current_path: Path,
    /// Handle to HelpStore state for dynamic updates on mount/unmount
    help_state: Option<HelpStoreHandle>,
}

impl StoreContext<CoreReplStoreFactory> {
    /// Create a new context with the default factory and standard mounts.
    ///
    /// This creates a context with the following mounts:
    /// - `/ctx/http` - Async HTTP broker (background execution)
    /// - `/ctx/http_sync` - Sync HTTP broker (blocking execution)
    /// - `/ctx/sys` - System utilities (time, env, proc, fs, random)
    /// - `/ctx/help` - Help system
    pub fn new() -> Self {
        Self::with_factory_and_mounts(CoreReplStoreFactory, true)
    }
}

/// Check if a path string refers to a register (starts with @)
pub fn is_register_path(path_str: &str) -> bool {
    path_str.starts_with('@')
}

/// Parse a register path into (register_name, sub_path)
pub fn parse_register_path(path_str: &str) -> Option<(String, Path)> {
    if !path_str.starts_with('@') {
        return None;
    }

    let without_at = &path_str[1..];
    if without_at.is_empty() {
        return Some(("".to_string(), Path::parse("").unwrap()));
    }

    if let Some(slash_pos) = without_at.find('/') {
        let name = &without_at[..slash_pos];
        let sub_path_str = &without_at[slash_pos + 1..];
        let sub_path = Path::parse(sub_path_str).ok()?;
        Some((name.to_string(), sub_path))
    } else {
        Some((without_at.to_string(), Path::parse("").unwrap()))
    }
}

impl<F: StoreFactory> StoreContext<F> {
    /// Create a context with a custom factory and optionally mount defaults.
    ///
    /// If `mount_defaults` is true, the standard mounts (http, sys, help, repl) are added.
    /// If false, the context starts with no mounts.
    pub fn with_factory_and_mounts(factory: F, mount_defaults: bool) -> Self {
        let mut store = MountStore::new(factory);
        let mut help_state: Option<HelpStoreHandle> = None;

        if mount_defaults {
            // Mount stores with docs FIRST (they create redirects)
            // Mount REPL docs store (REPL's own documentation)
            if let Err(e) = store.mount("ctx/repl", MountConfig::Repl) {
                eprintln!("Warning: Failed to mount REPL docs store: {}", e);
            }

            // Mount async HTTP broker (background execution)
            if let Err(e) = store.mount("ctx/http", MountConfig::AsyncHttpBroker) {
                eprintln!("Warning: Failed to mount async HTTP broker: {}", e);
            }

            // Mount sync HTTP broker (blocking execution)
            if let Err(e) = store.mount("ctx/http_sync", MountConfig::HttpBroker) {
                eprintln!("Warning: Failed to mount HTTP broker: {}", e);
            }

            // Mount sys store
            if let Err(e) = store.mount("ctx/sys", MountConfig::Sys) {
                eprintln!("Warning: Failed to mount sys store: {}", e);
            }

            // Mount register store (session-local named values)
            if let Err(e) = store.mount("ctx/registers", MountConfig::Registers) {
                eprintln!("Warning: Failed to mount register store: {}", e);
            }

            // Create shared state for HelpStore
            let state = Arc::new(RwLock::new(HelpStoreState::new()));
            help_state = Some(Arc::clone(&state));

            // Populate from existing redirects
            Self::populate_help_state(&mut store, &state);

            // Create HelpStore with shared state and mount it
            let help_store = HelpStore::with_shared_state(state);
            if let Err(e) = store.mount_store("ctx/help", Box::new(help_store)) {
                eprintln!("Warning: Failed to mount help store: {}", e);
            }
        }

        Self {
            store,
            current_path: Path::parse("").unwrap(),
            help_state,
        }
    }

    /// Populate HelpStore state from existing redirects.
    fn populate_help_state(store: &mut MountStore<F>, state: &HelpStoreHandle) {
        use structfs_core_store::path;

        let help_prefix = path!("ctx/help");
        let mut state_guard = state.write().unwrap();

        for (from, to, mode) in store.list_redirects() {
            // Only process redirects under /ctx/help
            if !from.has_prefix(&help_prefix) || from.len() <= 2 {
                continue;
            }

            // Extract topic name: /ctx/help/ctx/sys -> "ctx/sys"
            let topic = from.components[2..].join("/");

            // Try to read the docs manifest to get metadata for search
            let manifest = store
                .read(&to)
                .ok()
                .flatten()
                .and_then(|record| record.into_value(&structfs_core_store::NoCodec).ok());

            // Index the topic
            state_guard.index_docs(&topic, manifest);

            // Register redirect info for /ctx/help/meta
            state_guard.register_redirect(&topic, &format!("/{}", from), &format!("/{}", to), mode);
        }
    }

    /// Update HelpStore state after a mount/unmount operation.
    fn refresh_help_state(&mut self) {
        if let Some(ref state) = self.help_state {
            use structfs_core_store::path;

            let help_prefix = path!("ctx/help");
            let mut state_guard = state.write().unwrap();

            // Clear existing state
            state_guard.index = crate::help_store::DocsIndex::new();
            state_guard.redirects.clear();

            // Repopulate from current redirects
            for (from, to, mode) in self.store.list_redirects() {
                if !from.has_prefix(&help_prefix) || from.len() <= 2 {
                    continue;
                }

                let topic = from.components[2..].join("/");
                let manifest = self
                    .store
                    .read(&to)
                    .ok()
                    .flatten()
                    .and_then(|record| record.into_value(&structfs_core_store::NoCodec).ok());

                state_guard.index_docs(&topic, manifest);
                state_guard.register_redirect(
                    &topic,
                    &format!("/{}", from),
                    &format!("/{}", to),
                    mode,
                );
            }
        }
    }

    /// Create a minimal context with a custom factory and no default mounts.
    ///
    /// This is useful for testing when you want full control over what stores
    /// are mounted.
    pub fn with_factory(factory: F) -> Self {
        Self::with_factory_and_mounts(factory, false)
    }

    /// Mount a store at a path.
    ///
    /// This allows tests to add specific stores as needed.
    /// After mounting, the help index is refreshed to include any new docs.
    pub fn mount(&mut self, path: &str, config: MountConfig) -> Result<(), ContextError> {
        self.store.mount(path, config)?;
        self.refresh_help_state();
        Ok(())
    }

    /// Unmount a store at a path.
    ///
    /// After unmounting, the help index is refreshed to remove the store's docs.
    pub fn unmount(&mut self, path: &str) -> Result<(), ContextError> {
        self.store.unmount(path)?;
        self.refresh_help_state();
        Ok(())
    }

    /// Read from a register path.
    ///
    /// Reads from the mounted RegisterStore at `/ctx/registers/`.
    pub fn read_register(&mut self, path_str: &str) -> Result<Option<Value>, ContextError> {
        let (name, sub_path) = parse_register_path(path_str)
            .ok_or_else(|| ContextError::InvalidPath("Invalid register path".to_string()))?;

        // Build path under /ctx/registers/
        let register_path = if name.is_empty() {
            Path::parse("ctx/registers").unwrap()
        } else {
            let name_path = Path::parse(&name)
                .map_err(|e| ContextError::InvalidPath(format!("Invalid register name: {}", e)))?;
            Path::parse("ctx/registers")
                .unwrap()
                .join(&name_path)
                .join(&sub_path)
        };

        self.read(&register_path)
    }

    /// Write to a register path.
    ///
    /// Writes to the mounted RegisterStore at `/ctx/registers/`.
    pub fn write_register(&mut self, path_str: &str, value: Value) -> Result<Path, ContextError> {
        let (name, sub_path) = parse_register_path(path_str)
            .ok_or_else(|| ContextError::InvalidPath("Invalid register path".to_string()))?;

        if name.is_empty() {
            return Err(ContextError::InvalidPath(
                "Cannot write to register root. Use @name to specify a register.".to_string(),
            ));
        }

        let name_path = Path::parse(&name)
            .map_err(|e| ContextError::InvalidPath(format!("Invalid register name: {}", e)))?;
        let register_path = Path::parse("ctx/registers")
            .unwrap()
            .join(&name_path)
            .join(&sub_path);
        self.write(&register_path, value)
    }

    /// Store a value directly in a register by name.
    ///
    /// Convenience method that writes to `/ctx/registers/{name}`.
    pub fn set_register(&mut self, name: &str, value: Value) {
        let path = Path::parse(&format!("ctx/registers/{}", name)).unwrap();
        let _ = self.store.write(&path, Record::parsed(value));
    }

    /// Get a value from a register by name.
    ///
    /// Convenience method that reads from `/ctx/registers/{name}`.
    pub fn get_register(&mut self, name: &str) -> Option<Value> {
        let path = Path::parse(&format!("ctx/registers/{}", name)).unwrap();
        self.store
            .read(&path)
            .ok()
            .flatten()
            .and_then(|r| r.into_value(&NoCodec).ok())
    }

    /// List all register names.
    ///
    /// Reads from `/ctx/registers/` which returns an array of names.
    pub fn list_registers(&mut self) -> Vec<String> {
        let path = Path::parse("ctx/registers").unwrap();
        match self.store.read(&path) {
            Ok(Some(record)) => match record.into_value(&NoCodec) {
                Ok(Value::Array(arr)) => arr
                    .into_iter()
                    .filter_map(|v| match v {
                        Value::String(s) => Some(s),
                        _ => None,
                    })
                    .collect(),
                _ => vec![],
            },
            _ => vec![],
        }
    }

    /// Get the current path
    pub fn current_path(&self) -> &Path {
        &self.current_path
    }

    /// Set the current path
    pub fn set_current_path(&mut self, path: Path) {
        self.current_path = path;
    }

    /// Resolve a path relative to the current path
    pub fn resolve_path(&self, path_str: &str) -> Result<Path, ContextError> {
        if path_str.is_empty() || path_str == "." {
            return Ok(self.current_path.clone());
        }

        if path_str == "/" {
            return Ok(Path::parse("").unwrap());
        }

        if let Some(stripped) = path_str.strip_prefix('/') {
            Path::parse(stripped).map_err(|e| ContextError::InvalidPath(format!("{}", e)))
        } else if path_str == ".." {
            let mut components = self.current_path.components.clone();
            components.pop();
            Ok(Path { components })
        } else if path_str.starts_with("../") {
            let mut components = self.current_path.components.clone();
            let mut remaining = path_str;
            while remaining.starts_with("../") {
                components.pop();
                remaining = &remaining[3..];
            }
            if !remaining.is_empty() {
                let suffix = Path::parse(remaining)
                    .map_err(|e| ContextError::InvalidPath(format!("{}", e)))?;
                components.extend(suffix.components);
            }
            Ok(Path { components })
        } else {
            let suffix =
                Path::parse(path_str).map_err(|e| ContextError::InvalidPath(format!("{}", e)))?;
            Ok(self.current_path.join(&suffix))
        }
    }

    /// Read Value from a path
    pub fn read(&mut self, path: &Path) -> Result<Option<Value>, ContextError> {
        let record = self.store.read(path)?;
        match record {
            Some(r) => Ok(Some(r.into_value(&NoCodec)?)),
            None => Ok(None),
        }
    }

    /// Write Value to a path
    pub fn write(&mut self, path: &Path, value: Value) -> Result<Path, ContextError> {
        Ok(self.store.write(path, Record::parsed(value))?)
    }

    /// Read and convert to JsonValue for display compatibility
    pub fn read_as_json(&mut self, path: &Path) -> Result<Option<serde_json::Value>, ContextError> {
        match self.read(path)? {
            Some(value) => Ok(Some(value_to_json(value))),
            None => Ok(None),
        }
    }

    /// Write JsonValue (converts to Value internally)
    pub fn write_json(
        &mut self,
        path: &Path,
        json: &serde_json::Value,
    ) -> Result<Path, ContextError> {
        let value = json_to_value(json.clone());
        self.write(path, value)
    }
}

impl Default for StoreContext<CoreReplStoreFactory> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    // RegisterStore tests
    #[test]
    fn register_store_new() {
        let store = RegisterStore::new();
        assert!(store.list().is_empty());
    }

    #[test]
    fn register_store_default() {
        let store: RegisterStore = Default::default();
        assert!(store.list().is_empty());
    }

    #[test]
    fn register_store_get_set() {
        let mut store = RegisterStore::new();
        store.set("foo", Value::String("bar".to_string()));
        assert_eq!(store.get("foo"), Some(&Value::String("bar".to_string())));
        assert_eq!(store.get("nonexistent"), None);
    }

    #[test]
    fn register_store_list() {
        let mut store = RegisterStore::new();
        store.set("a", Value::Integer(1));
        store.set("b", Value::Integer(2));
        let list = store.list();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&&"a".to_string()));
        assert!(list.contains(&&"b".to_string()));
    }

    #[test]
    fn register_store_read_root() {
        let mut store = RegisterStore::new();
        store.set("x", Value::Integer(42));
        store.set("y", Value::Integer(99));
        let result = store.read(&path!("")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn register_store_read_register() {
        let mut store = RegisterStore::new();
        store.set("test", Value::String("hello".to_string()));
        let result = store.read(&path!("test")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("hello".to_string()));
    }

    #[test]
    fn register_store_read_nonexistent() {
        let mut store = RegisterStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn register_store_read_nested_map() {
        let mut store = RegisterStore::new();
        let mut map = BTreeMap::new();
        map.insert("inner".to_string(), Value::String("value".to_string()));
        store.set("outer", Value::Map(map));

        let result = store.read(&path!("outer/inner")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("value".to_string()));
    }

    #[test]
    fn register_store_read_nested_array() {
        let mut store = RegisterStore::new();
        store.set(
            "arr",
            Value::Array(vec![
                Value::Integer(10),
                Value::Integer(20),
                Value::Integer(30),
            ]),
        );

        let result = store.read(&path!("arr/1")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::Integer(20));
    }

    #[test]
    fn register_store_read_nested_invalid_path() {
        let mut store = RegisterStore::new();
        store.set("scalar", Value::Integer(42));
        let result = store.read(&path!("scalar/invalid")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn register_store_read_nested_array_invalid_index() {
        let mut store = RegisterStore::new();
        store.set("arr", Value::Array(vec![Value::Integer(1)]));
        let result = store.read(&path!("arr/notanumber")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn register_store_read_nested_array_out_of_bounds() {
        let mut store = RegisterStore::new();
        store.set("arr", Value::Array(vec![Value::Integer(1)]));
        let result = store.read(&path!("arr/100")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn register_store_write_register() {
        let mut store = RegisterStore::new();
        let result = store
            .write(&path!("newreg"), Record::parsed(Value::Integer(123)))
            .unwrap();
        assert_eq!(result.to_string(), "newreg");
        assert_eq!(store.get("newreg"), Some(&Value::Integer(123)));
    }

    #[test]
    fn register_store_write_root_error() {
        let mut store = RegisterStore::new();
        let result = store.write(&path!(""), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    // StoreContext tests
    #[test]
    fn test_register_write_read() {
        let mut ctx = StoreContext::new();
        ctx.set_register("foo", Value::String("bar".to_string()));
        let value = ctx.get_register("foo").unwrap();
        assert_eq!(value, Value::String("bar".to_string()));
    }

    #[test]
    fn test_register_list() {
        let mut ctx = StoreContext::new();
        ctx.set_register("a", Value::Integer(1));
        ctx.set_register("b", Value::Integer(2));
        let list = ctx.list_registers();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_register_via_path() {
        // Test that registers are accessible via /ctx/registers/ path
        let mut ctx = StoreContext::new();
        ctx.write(&path!("ctx/registers/test"), Value::Integer(42))
            .unwrap();
        let value = ctx.read(&path!("ctx/registers/test")).unwrap().unwrap();
        assert_eq!(value, Value::Integer(42));
    }

    #[test]
    fn test_register_list_via_path() {
        let mut ctx = StoreContext::new();
        ctx.write(&path!("ctx/registers/x"), Value::Integer(1))
            .unwrap();
        ctx.write(&path!("ctx/registers/y"), Value::Integer(2))
            .unwrap();
        let value = ctx.read(&path!("ctx/registers")).unwrap().unwrap();
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
                assert!(arr.contains(&Value::String("x".into())));
                assert!(arr.contains(&Value::String("y".into())));
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_resolve_absolute_path() {
        let ctx = StoreContext::new();
        let path = ctx.resolve_path("/foo/bar").unwrap();
        assert_eq!(path.to_string(), "foo/bar");
    }

    #[test]
    fn test_resolve_relative_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("foo").unwrap());
        let path = ctx.resolve_path("bar").unwrap();
        assert_eq!(path.to_string(), "foo/bar");
    }

    #[test]
    fn test_resolve_empty_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("foo").unwrap());
        let path = ctx.resolve_path("").unwrap();
        assert_eq!(path.to_string(), "foo");
    }

    #[test]
    fn test_resolve_dot_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("foo").unwrap());
        let path = ctx.resolve_path(".").unwrap();
        assert_eq!(path.to_string(), "foo");
    }

    #[test]
    fn test_resolve_root_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("foo/bar").unwrap());
        let path = ctx.resolve_path("/").unwrap();
        assert_eq!(path.to_string(), "");
    }

    #[test]
    fn test_resolve_parent_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("foo/bar").unwrap());
        let path = ctx.resolve_path("..").unwrap();
        assert_eq!(path.to_string(), "foo");
    }

    #[test]
    fn test_resolve_parent_relative_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("foo/bar/baz").unwrap());
        let path = ctx.resolve_path("../qux").unwrap();
        assert_eq!(path.to_string(), "foo/bar/qux");
    }

    #[test]
    fn test_resolve_multiple_parent_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(Path::parse("a/b/c/d").unwrap());
        let path = ctx.resolve_path("../../x").unwrap();
        assert_eq!(path.to_string(), "a/b/x");
    }

    #[test]
    fn test_current_path() {
        let mut ctx = StoreContext::new();
        assert_eq!(ctx.current_path().to_string(), "");
        ctx.set_current_path(Path::parse("foo/bar").unwrap());
        assert_eq!(ctx.current_path().to_string(), "foo/bar");
    }

    #[test]
    fn test_is_register_path() {
        assert!(is_register_path("@foo"));
        assert!(is_register_path("@foo/bar"));
        assert!(!is_register_path("/foo"));
        assert!(!is_register_path("foo"));
    }

    #[test]
    fn test_parse_register_path_simple() {
        let (name, sub) = parse_register_path("@foo").unwrap();
        assert_eq!(name, "foo");
        assert!(sub.is_empty());
    }

    #[test]
    fn test_parse_register_path_with_subpath() {
        let (name, sub) = parse_register_path("@foo/bar/baz").unwrap();
        assert_eq!(name, "foo");
        assert_eq!(sub.to_string(), "bar/baz");
    }

    #[test]
    fn test_parse_register_path_empty() {
        let (name, sub) = parse_register_path("@").unwrap();
        assert_eq!(name, "");
        assert!(sub.is_empty());
    }

    #[test]
    fn test_parse_register_path_not_register() {
        let result = parse_register_path("/foo");
        assert!(result.is_none());
    }

    #[test]
    fn test_read_register() {
        let mut ctx = StoreContext::new();
        ctx.set_register("test", Value::Integer(42));
        let value = ctx.read_register("@test").unwrap().unwrap();
        assert_eq!(value, Value::Integer(42));
    }

    #[test]
    fn test_read_register_not_found() {
        let mut ctx = StoreContext::new();
        let value = ctx.read_register("@nonexistent").unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn test_read_register_root() {
        let mut ctx = StoreContext::new();
        ctx.set_register("a", Value::Integer(1));
        let value = ctx.read_register("@").unwrap().unwrap();
        match value {
            Value::Array(arr) => assert_eq!(arr.len(), 1),
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_write_register() {
        let mut ctx = StoreContext::new();
        let path = ctx
            .write_register("@myvar", Value::String("value".to_string()))
            .unwrap();
        // Path returned includes ctx/registers/ prefix now
        assert!(path.to_string().contains("myvar"));
        assert_eq!(
            ctx.get_register("myvar"),
            Some(Value::String("value".to_string()))
        );
    }

    #[test]
    fn test_write_register_root_error() {
        let mut ctx = StoreContext::new();
        let result = ctx.write_register("@", Value::Null);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_sys_time() {
        let mut ctx = StoreContext::new();
        let value = ctx.read(&path!("ctx/sys/time/now")).unwrap();
        assert!(value.is_some());
        match value.unwrap() {
            Value::String(s) => assert!(s.contains("T")),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn test_read_help() {
        let mut ctx = StoreContext::new();
        let value = ctx.read(&path!("ctx/help")).unwrap();
        assert!(value.is_some());
        // HelpStore returns an array of indexed topic names
        match value.unwrap() {
            Value::Array(topics) => {
                // Should include topics from mounted stores with docs
                assert!(!topics.is_empty(), "Expected at least one help topic");
                // Should include sys (has docs)
                assert!(
                    topics.contains(&Value::String("ctx/sys".into())),
                    "Expected ctx/sys topic, got: {:?}",
                    topics
                );
                // Should include repl (has docs)
                assert!(
                    topics.contains(&Value::String("ctx/repl".into())),
                    "Expected ctx/repl topic"
                );
            }
            _ => panic!("Expected array of topics"),
        }
    }

    #[test]
    fn test_read_help_via_redirect() {
        let mut ctx = StoreContext::new();
        // Reading through the redirect should work
        let value = ctx.read(&path!("ctx/help/ctx/sys")).unwrap();
        assert!(value.is_some());
        // Should get sys docs via redirect
        match value.unwrap() {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
            }
            _ => panic!("Expected sys docs map"),
        }
    }

    #[test]
    fn test_read_repl_docs() {
        let mut ctx = StoreContext::new();
        // Read REPL docs directly
        let value = ctx.read(&path!("ctx/repl/docs")).unwrap();
        assert!(value.is_some());
        match value.unwrap() {
            Value::Map(map) => {
                assert_eq!(
                    map.get("title"),
                    Some(&Value::String("REPL Documentation".into()))
                );
            }
            _ => panic!("Expected REPL docs manifest"),
        }
    }

    #[test]
    fn test_read_help_meta() {
        let mut ctx = StoreContext::new();
        let value = ctx.read(&path!("ctx/help/meta")).unwrap();
        assert!(value.is_some());
        match value.unwrap() {
            Value::Array(redirects) => {
                // Should have redirects for stores with docs
                assert!(!redirects.is_empty());
                // Each redirect should have topic, from, to, mode
                if let Value::Map(first) = &redirects[0] {
                    assert!(first.contains_key("topic"));
                    assert!(first.contains_key("from"));
                    assert!(first.contains_key("to"));
                    assert!(first.contains_key("mode"));
                }
            }
            _ => panic!("Expected array of redirects"),
        }
    }

    #[test]
    fn test_read_help_search() {
        let mut ctx = StoreContext::new();
        // Search for "time" should find sys (which has time operations)
        let value = ctx.read(&path!("ctx/help/search/System")).unwrap();
        assert!(value.is_some());
        match value.unwrap() {
            Value::Map(result) => {
                assert_eq!(result.get("query"), Some(&Value::String("System".into())));
                // Should find at least sys (title is "System Primitives")
                if let Some(Value::Integer(count)) = result.get("count") {
                    assert!(*count > 0, "Expected search to find results");
                }
            }
            _ => panic!("Expected search result map"),
        }
    }

    #[test]
    fn test_dynamic_unmount_removes_help_topic() {
        let mut ctx = StoreContext::new();

        // Get initial topic count
        let initial_topics = match ctx.read(&path!("ctx/help")).unwrap().unwrap() {
            Value::Array(arr) => arr.len(),
            _ => panic!("Expected array"),
        };

        // Unmount the sys store (has docs)
        ctx.unmount("ctx/sys").unwrap();
        let after_unmount = match ctx.read(&path!("ctx/help")).unwrap().unwrap() {
            Value::Array(arr) => arr.len(),
            _ => panic!("Expected array"),
        };
        assert!(
            after_unmount < initial_topics,
            "Unmounting sys should remove its help topic"
        );

        // Verify ctx/sys is no longer in the topic list
        let topics = ctx.read(&path!("ctx/help")).unwrap().unwrap();
        match topics {
            Value::Array(arr) => {
                assert!(
                    !arr.contains(&Value::String("ctx/sys".into())),
                    "ctx/sys should not be in topics after unmount"
                );
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn test_dynamic_mount_adds_help_topic() {
        let mut ctx = StoreContext::new();

        // First unmount sys so we can remount it
        ctx.unmount("ctx/sys").unwrap();

        // Verify ctx/sys is NOT in topics
        let topics_before = ctx.read(&path!("ctx/help")).unwrap().unwrap();
        let count_before = match &topics_before {
            Value::Array(arr) => {
                assert!(
                    !arr.contains(&Value::String("ctx/sys".into())),
                    "ctx/sys should not be in topics after unmount"
                );
                arr.len()
            }
            _ => panic!("Expected array"),
        };

        // Remount sys (which has docs)
        ctx.mount("ctx/sys", MountConfig::Sys).unwrap();

        // Verify ctx/sys IS now in topics
        let topics_after = ctx.read(&path!("ctx/help")).unwrap().unwrap();
        match topics_after {
            Value::Array(arr) => {
                assert!(
                    arr.contains(&Value::String("ctx/sys".into())),
                    "ctx/sys should be in topics after mount"
                );
                assert_eq!(
                    arr.len(),
                    count_before + 1,
                    "Topic count should increase by 1"
                );
            }
            _ => panic!("Expected array"),
        }

        // Verify we can read the docs via help redirect
        let docs = ctx.read(&path!("ctx/help/ctx/sys")).unwrap();
        assert!(docs.is_some(), "Should be able to read sys docs via help");
        match docs.unwrap() {
            Value::Map(map) => {
                assert!(map.contains_key("title"), "Sys docs should have title");
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_read_as_json() {
        let mut ctx = StoreContext::new();
        let json = ctx
            .read_as_json(&path!("ctx/sys/time/now_unix"))
            .unwrap()
            .unwrap();
        assert!(json.is_number());
    }

    #[test]
    fn test_read_as_json_not_found() {
        let mut ctx = StoreContext::new();
        // Use a path under a valid mount but that doesn't exist
        let json = ctx
            .read_as_json(&path!("ctx/sys/env/NONEXISTENT_ENV_VAR_12345"))
            .unwrap();
        assert!(json.is_none());
    }

    #[test]
    fn test_write_json() {
        let mut ctx = StoreContext::new();
        ctx.mount("test", MountConfig::Memory).unwrap();
        let json = serde_json::json!({"key": "value"});
        ctx.write_json(&path!("test/data"), &json).unwrap();
        let result = ctx.read_as_json(&path!("test/data")).unwrap().unwrap();
        assert_eq!(result, json);
    }

    #[test]
    fn test_with_factory_no_mounts() {
        let ctx = StoreContext::with_factory(CoreReplStoreFactory);
        // Should not have default mounts
        let result = ctx.resolve_path("/ctx/sys").unwrap();
        assert_eq!(result.to_string(), "ctx/sys");
    }

    #[test]
    fn test_with_factory_and_mounts_false() {
        let ctx = StoreContext::with_factory_and_mounts(CoreReplStoreFactory, false);
        // No default mounts
        let result = ctx.resolve_path("/").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_mount() {
        let mut ctx = StoreContext::with_factory(CoreReplStoreFactory);
        ctx.mount("mystore", MountConfig::Memory).unwrap();
        ctx.write(&path!("mystore/key"), Value::Integer(123))
            .unwrap();
        let value = ctx.read(&path!("mystore/key")).unwrap().unwrap();
        assert_eq!(value, Value::Integer(123));
    }

    #[test]
    fn test_default_impl() {
        let ctx: StoreContext = Default::default();
        assert!(ctx.current_path().is_empty());
    }

    #[test]
    fn context_error_display() {
        let err = ContextError::InvalidPath("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }

    // Factory error path tests
    #[test]
    fn factory_local_not_available() {
        let factory = CoreReplStoreFactory;
        let result = factory.create(&MountConfig::Local {
            path: "/tmp".to_string(),
        });
        match result {
            Err(e) => assert!(e.to_string().contains("not yet available")),
            Ok(_) => panic!("Expected error for Local config"),
        }
    }

    #[test]
    fn factory_http_not_available() {
        let factory = CoreReplStoreFactory;
        let result = factory.create(&MountConfig::Http {
            url: "https://example.com".to_string(),
        });
        match result {
            Err(e) => assert!(e.to_string().contains("not yet available")),
            Ok(_) => panic!("Expected error for Http config"),
        }
    }

    #[test]
    fn factory_structfs_not_available() {
        let factory = CoreReplStoreFactory;
        let result = factory.create(&MountConfig::Structfs {
            url: "https://example.com".to_string(),
        });
        match result {
            Err(e) => assert!(e.to_string().contains("not yet available")),
            Ok(_) => panic!("Expected error for Structfs config"),
        }
    }

    #[test]
    fn factory_creates_memory_store() {
        let factory = CoreReplStoreFactory;
        let result = factory.create(&MountConfig::Memory);
        assert!(result.is_ok());
    }

    #[test]
    fn factory_creates_registers_store() {
        let factory = CoreReplStoreFactory;
        let result = factory.create(&MountConfig::Registers);
        assert!(result.is_ok());
    }

    #[test]
    fn list_registers_empty() {
        let mut ctx = StoreContext::new();
        let list = ctx.list_registers();
        assert!(list.is_empty());
    }
}
