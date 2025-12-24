//! Store context for the REPL.
//!
//! The REPL uses a MountStore as its root, which allows mounting other stores
//! via writes to `/_mounts/*`. This provides uniform access - everything is
//! managed through read/write operations.

use std::path::PathBuf;

use serde_json::Value as JsonValue;

use structfs_http::blocking::HttpClientStore;
use structfs_http::broker::HttpBrokerStore;
use structfs_http::RemoteStore;
use structfs_json_store::in_memory::SerdeJSONInMemoryStore;
use structfs_json_store::JSONLocalStore;
use structfs_store::{
    Error as StoreError, MountConfig, MountStore, Path, Reader, StoreBox, StoreFactory, Writer,
};

use crate::help_store::HelpStore;

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
            MountConfig::Structfs { url } => {
                let store = RemoteStore::new(url).map_err(|e| StoreError::Raw {
                    message: format!("Failed to connect to remote StructFS at '{}': {}", url, e),
                })?;
                Ok(StoreBox::new(store))
            }
            MountConfig::Help => Ok(StoreBox::new(HelpStore::new())),
        }
    }
}

/// Manages the REPL's root store and current path
pub struct StoreContext {
    store: MountStore<ReplStoreFactory>,
    current_path: Path,
}

impl StoreContext {
    pub fn new() -> Self {
        let mut store = MountStore::new(ReplStoreFactory);

        // Set up default mounts under /ctx
        if let Err(e) = store.mount("ctx/http", MountConfig::HttpBroker) {
            eprintln!("Warning: Failed to mount default HTTP broker: {}", e);
        }
        if let Err(e) = store.mount("ctx/help", MountConfig::Help) {
            eprintln!("Warning: Failed to mount help store: {}", e);
        }

        Self {
            store,
            current_path: Path::parse("").unwrap(),
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
