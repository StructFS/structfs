//! New architecture store context for the REPL.
//!
//! This module provides the store context using the new three-layer architecture
//! (ll-store, core-store, serde-store) instead of the legacy erased_serde approach.

use std::collections::BTreeMap;

use structfs_core_store::{
    mount_store::{MountConfig, MountStore, StoreFactory},
    overlay_store::StoreBox,
    Error as CoreError, NoCodec, Path, Reader, Record, Value, Writer,
};

use structfs_serde_store::{json_to_value, value_to_json};

// Import the new store implementations
use structfs_http::core as http_core;
use structfs_json_store::InMemoryStore;
use structfs_sys::core as sys_core;

#[derive(thiserror::Error, Debug)]
pub enum ContextError {
    #[error("Store error: {0}")]
    Store(#[from] CoreError),

    #[error("HTTP error: {0}")]
    Http(#[from] structfs_http::Error),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Factory for creating stores from mount configurations (new architecture)
struct CoreReplStoreFactory;

impl StoreFactory for CoreReplStoreFactory {
    fn create(&self, config: &MountConfig) -> Result<StoreBox, CoreError> {
        match config {
            MountConfig::Memory => Ok(Box::new(InMemoryStore::new())),
            MountConfig::Local { path: _ } => {
                // Local disk store not yet migrated to new architecture
                Err(CoreError::Other {
                    message: "Local disk store not yet available in new architecture".to_string(),
                })
            }
            MountConfig::Http { url: _ } => {
                // HTTP client not using direct mode in REPL context
                Err(CoreError::Other {
                    message: "HTTP client store not yet available in new architecture".to_string(),
                })
            }
            MountConfig::HttpBroker => {
                let store = http_core::HttpBrokerStore::with_default_timeout().map_err(|e| {
                    CoreError::Other {
                        message: format!("Failed to create HTTP broker: {}", e),
                    }
                })?;
                Ok(Box::new(store))
            }
            MountConfig::AsyncHttpBroker => {
                // Async broker uses threads internally, not migrated yet
                Err(CoreError::Other {
                    message: "Async HTTP broker not yet available in new architecture".to_string(),
                })
            }
            MountConfig::Structfs { url: _ } => Err(CoreError::Other {
                message: "Remote StructFS not yet available in new architecture".to_string(),
            }),
            MountConfig::Help => Err(CoreError::Other {
                message: "Help store not yet available in new architecture".to_string(),
            }),
            MountConfig::Sys => Ok(Box::new(sys_core::SysStore::new())),
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
            return Err(CoreError::Other {
                message: "Cannot write to register root. Use @name to specify a register."
                    .to_string(),
            });
        }

        let value = data.into_value(&NoCodec)?;
        let register_name = &to[0];

        // For simplicity, we only support writing to the register itself (not sub-paths)
        // Full sub-path support could be added later
        self.registers.insert(register_name.to_string(), value);
        Ok(to.clone())
    }
}

/// Store context using the new architecture
pub struct StoreContext {
    store: MountStore<CoreReplStoreFactory>,
    registers: RegisterStore,
    current_path: Path,
}

impl StoreContext {
    pub fn new() -> Self {
        let mut store = MountStore::new(CoreReplStoreFactory);

        // Mount sync HTTP broker
        if let Err(e) = store.mount("ctx/http_sync", MountConfig::HttpBroker) {
            eprintln!("Warning: Failed to mount HTTP broker: {}", e);
        }

        // Mount sys store
        if let Err(e) = store.mount("ctx/sys", MountConfig::Sys) {
            eprintln!("Warning: Failed to mount sys store: {}", e);
        }

        Self {
            store,
            registers: RegisterStore::new(),
            current_path: Path::parse("").unwrap(),
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

    /// Read from a register path
    pub fn read_register(&mut self, path_str: &str) -> Result<Option<Value>, ContextError> {
        let (name, sub_path) = Self::parse_register_path(path_str)
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
        let (name, sub_path) = Self::parse_register_path(path_str)
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

impl Default for StoreContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

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
    fn test_read_sys_time() {
        let mut ctx = StoreContext::new();
        let value = ctx.read(&path!("ctx/sys/time/now")).unwrap();
        assert!(value.is_some());
        match value.unwrap() {
            Value::String(s) => assert!(s.contains("T")),
            _ => panic!("Expected string"),
        }
    }
}
