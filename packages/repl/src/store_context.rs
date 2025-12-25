//! Store context for the REPL.
//!
//! The REPL uses a MountStore as its root, which allows mounting other stores
//! via writes to `/_mounts/*`. This provides uniform access - everything is
//! managed through read/write operations.

use std::path::PathBuf;

use serde_json::Value as JsonValue;

use structfs_http::async_broker::AsyncHttpBrokerStore;
use structfs_http::blocking::HttpClientStore;
use structfs_http::broker::HttpBrokerStore;
use structfs_http::RemoteStore;
use structfs_json_store::in_memory::SerdeJSONInMemoryStore;
use structfs_json_store::JSONLocalStore;
use structfs_store::{
    Error as StoreError, MountConfig, MountStore, Path, Reader, StoreBox, StoreFactory, Writer,
};
use structfs_sys::{DocsStore as SysDocsStore, SysStore};

use crate::help_store::HelpStore;
use crate::register_store::RegisterStore;

#[derive(thiserror::Error, Debug)]
pub enum ContextError {
    #[error("Store error: {0}")]
    Store(#[from] StoreError),

    #[error("HTTP error: {0}")]
    Http(#[from] structfs_http::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Local store error: {0}")]
    LocalStore(#[from] structfs_store::LocalStoreError),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Factory for creating stores from mount configurations
struct ReplStoreFactory;

impl StoreFactory for ReplStoreFactory {
    fn create(&self, config: &MountConfig) -> Result<StoreBox<'static>, StoreError> {
        match config {
            MountConfig::Memory => {
                let store = SerdeJSONInMemoryStore::new().map_err(|e| StoreError::Raw {
                    message: format!("Failed to create memory store: {}", e),
                })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::Local { path } => {
                let store =
                    JSONLocalStore::new(PathBuf::from(path)).map_err(|e| StoreError::Raw {
                        message: format!("Failed to open local store at '{}': {}", path, e),
                    })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::Http { url } => {
                let store = HttpClientStore::new(url).map_err(|e| StoreError::Raw {
                    message: format!("Failed to create HTTP client for '{}': {}", url, e),
                })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::HttpBroker => {
                let store =
                    HttpBrokerStore::with_default_timeout().map_err(|e| StoreError::Raw {
                        message: format!("Failed to create HTTP broker: {}", e),
                    })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::AsyncHttpBroker => {
                let store =
                    AsyncHttpBrokerStore::with_default_timeout().map_err(|e| StoreError::Raw {
                        message: format!("Failed to create async HTTP broker: {}", e),
                    })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::Structfs { url } => {
                let store = RemoteStore::new(url).map_err(|e| StoreError::Raw {
                    message: format!("Failed to connect to remote StructFS at '{}': {}", url, e),
                })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::Help => Ok(StoreBox::new(HelpStore::new())),
            MountConfig::Sys => Ok(StoreBox::new(SysStore::new())),
        }
    }
}

/// Manages the REPL's root store and current path
pub struct StoreContext {
    store: MountStore<ReplStoreFactory>,
    registers: RegisterStore,
    current_path: Path,
}

impl StoreContext {
    pub fn new() -> Self {
        let mut store = MountStore::new(ReplStoreFactory);

        // Set up default mounts under /ctx
        // Async HTTP broker - requests execute in background, can have multiple outstanding
        if let Err(e) = store.mount("ctx/http", MountConfig::AsyncHttpBroker) {
            eprintln!("Warning: Failed to mount async HTTP broker: {}", e);
        }
        // Sync HTTP broker - blocks until request completes on read
        if let Err(e) = store.mount("ctx/http_sync", MountConfig::HttpBroker) {
            eprintln!("Warning: Failed to mount sync HTTP broker: {}", e);
        }

        // System primitives (env, time, proc, fs, random)
        if let Err(e) = store.mount("ctx/sys", MountConfig::Sys) {
            eprintln!("Warning: Failed to mount sys store: {}", e);
        }

        // Create help store with mounted docs from other stores
        let mut help_store = HelpStore::new();
        // Mount sys docs into help store so `read /ctx/help/sys` returns sys documentation
        help_store.mount_docs("sys", SysDocsStore::new());

        // Mount the configured help store (bypasses factory since it has dependencies)
        if let Err(e) = store.mount_store("ctx/help", StoreBox::new(help_store)) {
            eprintln!("Warning: Failed to mount help store: {}", e);
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
    /// For example, "@foo/bar/baz" -> ("foo", Some(Path with ["bar", "baz"]))
    pub fn parse_register_path(path_str: &str) -> Option<(String, Path)> {
        if !path_str.starts_with('@') {
            return None;
        }

        let without_at = &path_str[1..];
        if without_at.is_empty() {
            // Just "@" - return empty register name (list all registers)
            return Some(("".to_string(), Path::parse("").unwrap()));
        }

        // Split by first '/'
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
    pub fn read_register(&mut self, path_str: &str) -> Result<Option<JsonValue>, ContextError> {
        let (name, sub_path) = Self::parse_register_path(path_str)
            .ok_or_else(|| ContextError::InvalidPath("Invalid register path".to_string()))?;

        // Build the full path for the register store (register name + sub path)
        let full_path = if name.is_empty() {
            sub_path
        } else {
            let name_path = Path::parse(&name)
                .map_err(|e| ContextError::InvalidPath(format!("Invalid register name: {}", e)))?;
            name_path.join(&sub_path)
        };

        Ok(self.registers.read_owned(&full_path)?)
    }

    /// Write to a register path
    pub fn write_register(
        &mut self,
        path_str: &str,
        value: &JsonValue,
    ) -> Result<Path, ContextError> {
        let (name, sub_path) = Self::parse_register_path(path_str)
            .ok_or_else(|| ContextError::InvalidPath("Invalid register path".to_string()))?;

        if name.is_empty() {
            return Err(ContextError::InvalidPath(
                "Cannot write to register root. Use @name to specify a register.".to_string(),
            ));
        }

        // Build the full path for the register store
        let name_path = Path::parse(&name)
            .map_err(|e| ContextError::InvalidPath(format!("Invalid register name: {}", e)))?;
        let full_path = name_path.join(&sub_path);
        Ok(self.registers.write(&full_path, value)?)
    }

    /// Store a value directly in a register by name
    pub fn set_register(&mut self, name: &str, value: JsonValue) {
        self.registers.set(name, value);
    }

    /// Get a value from a register by name
    pub fn get_register(&self, name: &str) -> Option<&JsonValue> {
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

    /// Get list of current mounts for display
    pub fn list_mounts(&self) -> Vec<structfs_store::MountInfo> {
        self.store.list_mounts()
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
            // Absolute path
            Path::parse(stripped).map_err(|e| ContextError::InvalidPath(format!("{}", e)))
        } else if path_str == ".." {
            // Go up one level
            let mut components = self.current_path.components.clone();
            components.pop();
            Ok(Path { components })
        } else if path_str.starts_with("../") {
            // Relative path going up
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
            // Relative path
            let suffix =
                Path::parse(path_str).map_err(|e| ContextError::InvalidPath(format!("{}", e)))?;
            Ok(self.current_path.join(&suffix))
        }
    }

    /// Read JSON from a path
    pub fn read(&mut self, path: &Path) -> Result<Option<JsonValue>, ContextError> {
        Ok(self.store.read_owned(path)?)
    }

    /// Write JSON to a path
    pub fn write(&mut self, path: &Path, value: &JsonValue) -> Result<Path, ContextError> {
        Ok(self.store.write(path, value)?)
    }
}

impl Default for StoreContext {
    fn default() -> Self {
        Self::new()
    }
}
