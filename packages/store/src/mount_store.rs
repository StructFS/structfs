//! A store that manages mounts through read/write operations.
//!
//! This store exposes mount management through the StructFS interface itself:
//! - Read `/ctx/mounts` to list all mounts
//! - Write to `/ctx/mounts/<name>` to create a mount at `/<name>`
//! - Write `null` to `/ctx/mounts/<name>` to unmount
//!
//! Mount configurations are JSON objects like:
//! ```json
//! {"type": "memory"}
//! {"type": "local", "path": "/path/to/dir"}
//! {"type": "http", "url": "https://api.example.com"}
//! {"type": "structfs", "url": "https://structfs.example.com"}
//! ```

use std::collections::HashMap;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::overlay_store::{OverlayStore, StoreBox};
use crate::store::{Error as StoreError, Path, Reader, Writer};

/// Configuration for a mount point
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MountConfig {
    /// In-memory JSON store
    Memory,
    /// Local filesystem JSON store
    Local { path: String },
    /// HTTP client store (for making HTTP requests to a base URL)
    Http { url: String },
    /// HTTP broker store - write HttpRequest, read from handle to execute (sync)
    HttpBroker,
    /// Async HTTP broker - executes requests in background threads
    AsyncHttpBroker,
    /// Remote StructFS store over HTTP
    Structfs { url: String },
    /// Help/documentation store (read-only)
    Help,
}

/// Information about a mount point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountInfo {
    pub path: String,
    pub config: MountConfig,
}

/// A factory for creating stores from mount configurations
pub trait StoreFactory: Send + Sync {
    fn create(&self, config: &MountConfig) -> Result<StoreBox<'static>, StoreError>;
}

/// A store that manages mounts through read/write operations
pub struct MountStore<F: StoreFactory> {
    overlay: OverlayStore<'static>,
    mounts: HashMap<String, MountConfig>,
    factory: F,
}

const MOUNTS_PREFIX: [&str; 2] = ["ctx", "mounts"];

impl<F: StoreFactory> MountStore<F> {
    pub fn new(factory: F) -> Self {
        Self {
            overlay: OverlayStore::default(),
            mounts: HashMap::new(),
            factory,
        }
    }

    /// Mount a store at the given path
    pub fn mount(&mut self, name: &str, config: MountConfig) -> Result<(), StoreError> {
        // Create the store from the config
        let store = self.factory.create(&config)?;

        // Parse the mount path
        let mount_path = Path::parse(name).map_err(StoreError::PathError)?;

        // Add to overlay
        self.overlay.add_layer(mount_path, store).map_err(|e| {
            StoreError::ImplementationFailure {
                message: e.to_string(),
            }
        })?;

        // Track the mount
        self.mounts.insert(name.to_string(), config);

        Ok(())
    }

    /// Unmount a store at the given path
    pub fn unmount(&mut self, name: &str) -> Result<(), StoreError> {
        if !self.mounts.contains_key(name) {
            return Err(StoreError::Raw {
                message: format!("No mount at '{}'", name),
            });
        }

        // Remove from tracking
        self.mounts.remove(name);

        // Note: OverlayStore.remove_layer is not fully implemented
        // For now, we mask the path
        let mount_path = Path::parse(name).map_err(StoreError::PathError)?;
        self.overlay
            .mask_sub_tree(mount_path)
            .map_err(|e| StoreError::ImplementationFailure {
                message: e.to_string(),
            })?;

        Ok(())
    }

    /// List all mounts
    pub fn list_mounts(&self) -> Vec<MountInfo> {
        self.mounts
            .iter()
            .map(|(path, config)| MountInfo {
                path: path.clone(),
                config: config.clone(),
            })
            .collect()
    }

    fn is_mounts_path(path: &Path) -> bool {
        path.components.len() >= 2
            && path.components[0] == MOUNTS_PREFIX[0]
            && path.components[1] == MOUNTS_PREFIX[1]
    }

    fn get_mount_name(path: &Path) -> Option<String> {
        if path.components.len() >= 3
            && path.components[0] == MOUNTS_PREFIX[0]
            && path.components[1] == MOUNTS_PREFIX[1]
        {
            Some(path.components[2..].join("/"))
        } else {
            None
        }
    }
}

impl<F: StoreFactory> Reader for MountStore<F> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        if Self::is_mounts_path(from) {
            // Handle reads to /ctx/mounts/*
            if from.components.len() == 2 {
                // Reading /ctx/mounts - return list of mounts
                let mounts = self.list_mounts();
                let json =
                    serde_json::to_value(&mounts).map_err(|e| StoreError::RecordSerialization {
                        message: e.to_string(),
                    })?;
                return Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
                    json,
                ))));
            } else if let Some(name) = Self::get_mount_name(from) {
                // Reading /ctx/mounts/<name> - return mount config
                if let Some(config) = self.mounts.get(&name) {
                    let json = serde_json::to_value(config).map_err(|e| {
                        StoreError::RecordSerialization {
                            message: e.to_string(),
                        }
                    })?;
                    return Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
                        json,
                    ))));
                } else {
                    return Ok(None);
                }
            }
        }

        // Delegate to overlay
        self.overlay.read_to_deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        if Self::is_mounts_path(from) {
            // Handle reads to /ctx/mounts/*
            if from.components.len() == 2 {
                // Reading /ctx/mounts - return list of mounts
                let mounts = self.list_mounts();
                let json =
                    serde_json::to_value(&mounts).map_err(|e| StoreError::RecordSerialization {
                        message: e.to_string(),
                    })?;
                let record = serde_json::from_value(json).map_err(|e| {
                    StoreError::RecordDeserialization {
                        message: e.to_string(),
                    }
                })?;
                return Ok(Some(record));
            } else if let Some(name) = Self::get_mount_name(from) {
                // Reading /ctx/mounts/<name> - return mount config
                if let Some(config) = self.mounts.get(&name) {
                    let json = serde_json::to_value(config).map_err(|e| {
                        StoreError::RecordSerialization {
                            message: e.to_string(),
                        }
                    })?;
                    let record = serde_json::from_value(json).map_err(|e| {
                        StoreError::RecordDeserialization {
                            message: e.to_string(),
                        }
                    })?;
                    return Ok(Some(record));
                } else {
                    return Ok(None);
                }
            }
        }

        // Delegate to overlay
        self.overlay.read_owned(from)
    }
}

impl<F: StoreFactory> Writer for MountStore<F> {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        if Self::is_mounts_path(destination) {
            // Handle writes to /ctx/mounts/*
            if let Some(name) = Self::get_mount_name(destination) {
                // Convert data to JSON Value to check for null
                let json =
                    serde_json::to_value(&data).map_err(|e| StoreError::RecordSerialization {
                        message: e.to_string(),
                    })?;

                if json.is_null() {
                    // Unmount
                    self.unmount(&name)?;
                } else {
                    // Parse as MountConfig and mount
                    let config: MountConfig = serde_json::from_value(json).map_err(|e| {
                        StoreError::RecordDeserialization {
                            message: format!("Invalid mount config: {}", e),
                        }
                    })?;
                    self.mount(&name, config)?;
                }
                return Ok(destination.clone());
            } else {
                return Err(StoreError::Raw {
                    message: "Cannot write directly to /ctx/mounts".to_string(),
                });
            }
        }

        // Delegate to overlay
        self.overlay.write(destination, data)
    }
}

// Tests are in packages/json_store/tests/ to avoid circular dependency
