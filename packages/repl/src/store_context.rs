//! Store context for the REPL.
//!
//! This module provides the store context that manages mounts and registers.

use std::collections::BTreeMap;

use structfs_core_store::{
    mount_store::{MountConfig, MountStore, StoreFactory},
    overlay_store::StoreBox,
    Error as CoreError, NoCodec, Path, Reader, Record, Value, Writer,
};

use structfs_serde_store::{json_to_value, value_to_json};

// Import store implementations
use crate::help_store::HelpStore;
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
pub struct StoreContext<F: StoreFactory = CoreReplStoreFactory> {
    store: MountStore<F>,
    registers: RegisterStore,
    current_path: Path,
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
    /// If `mount_defaults` is true, the standard mounts (http, sys, help) are added.
    /// If false, the context starts with no mounts.
    pub fn with_factory_and_mounts(factory: F, mount_defaults: bool) -> Self {
        let mut store = MountStore::new(factory);

        if mount_defaults {
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

            // Mount help store
            if let Err(e) = store.mount("ctx/help", MountConfig::Help) {
                eprintln!("Warning: Failed to mount help store: {}", e);
            }
        }

        Self {
            store,
            registers: RegisterStore::new(),
            current_path: Path::parse("").unwrap(),
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
    pub fn mount(&mut self, path: &str, config: MountConfig) -> Result<(), ContextError> {
        self.store.mount(path, config)?;
        Ok(())
    }

    /// Read from a register path
    pub fn read_register(&mut self, path_str: &str) -> Result<Option<Value>, ContextError> {
        let (name, sub_path) = parse_register_path(path_str)
            .ok_or_else(|| ContextError::InvalidPath("Invalid register path".to_string()))?;

        let full_path = if name.is_empty() {
            sub_path
        } else {
            let name_path = Path::parse(&name)
                .map_err(|e| ContextError::InvalidPath(format!("Invalid register name: {}", e)))?;
            name_path.join(&sub_path)
        };

        let record = self.registers.read(&full_path)?;
        match record {
            Some(r) => Ok(Some(r.into_value(&NoCodec)?)),
            None => Ok(None),
        }
    }

    /// Write to a register path
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
        let full_path = name_path.join(&sub_path);
        Ok(self.registers.write(&full_path, Record::parsed(value))?)
    }

    /// Store a value directly in a register by name
    pub fn set_register(&mut self, name: &str, value: Value) {
        self.registers.set(name, value);
    }

    /// Get a value from a register by name
    pub fn get_register(&self, name: &str) -> Option<&Value> {
        self.registers.get(name)
    }

    /// List all register names
    pub fn list_registers(&self) -> Vec<&String> {
        self.registers.list()
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
        assert_eq!(value, &Value::String("bar".to_string()));
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
        assert_eq!(path.to_string(), "myvar");
        assert_eq!(
            ctx.get_register("myvar"),
            Some(&Value::String("value".to_string()))
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
        match value.unwrap() {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("topics"));
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
}
