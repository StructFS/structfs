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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::overlay_store::{OverlayStore, StoreBox};
use crate::{Error, Path, Reader, Record, Value, Writer};

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
    /// System primitives store (env, time, proc, fs, random)
    Sys,
}

/// Information about a mount point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountInfo {
    pub path: String,
    pub config: MountConfig,
}

/// A factory for creating stores from mount configurations
pub trait StoreFactory: Send + Sync {
    fn create(&self, config: &MountConfig) -> Result<StoreBox, Error>;
}

/// A store that manages mounts through read/write operations
pub struct MountStore<F: StoreFactory> {
    overlay: OverlayStore,
    mounts: BTreeMap<String, MountConfig>,
    factory: F,
}

const MOUNTS_PREFIX: [&str; 2] = ["ctx", "mounts"];

impl<F: StoreFactory> MountStore<F> {
    pub fn new(factory: F) -> Self {
        Self {
            overlay: OverlayStore::new(),
            mounts: BTreeMap::new(),
            factory,
        }
    }

    /// Mount a store at the given path
    pub fn mount(&mut self, name: &str, config: MountConfig) -> Result<(), Error> {
        // Create the store from the config
        let store = self.factory.create(&config)?;

        // Parse the mount path
        let mount_path = Path::parse(name).map_err(Error::Path)?;

        // Add to overlay
        self.overlay.add_layer(mount_path, store);

        // Track the mount
        self.mounts.insert(name.to_string(), config);

        Ok(())
    }

    /// Mount a pre-created store at the given path.
    ///
    /// This bypasses the factory and allows mounting stores that have
    /// complex initialization requirements (e.g., cross-store dependencies).
    pub fn mount_store(&mut self, name: &str, store: StoreBox) -> Result<(), Error> {
        // Parse the mount path
        let mount_path = Path::parse(name).map_err(Error::Path)?;

        // Add to overlay
        self.overlay.add_layer(mount_path, store);

        // Don't track in mounts BTreeMap since we don't have a config
        // This mount won't show up in list_mounts or be serializable,
        // which is fine for built-in stores like help

        Ok(())
    }

    /// Unmount a store at the given path
    pub fn unmount(&mut self, name: &str) -> Result<(), Error> {
        if !self.mounts.contains_key(name) {
            return Err(Error::Other {
                message: format!("No mount at '{}'", name),
            });
        }

        // Remove from tracking
        self.mounts.remove(name);

        // Note: For now we just remove from tracking.
        // A full implementation would need to remove from overlay.
        // This is a limitation carried over from the legacy implementation.

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

    /// Convert MountInfo list to Value
    fn mounts_to_value(&self) -> Value {
        let mounts = self.list_mounts();
        let mut arr = Vec::with_capacity(mounts.len());
        for info in mounts {
            let mut map = BTreeMap::new();
            map.insert("path".to_string(), Value::String(info.path));
            // Serialize config to a map
            let config_value = config_to_value(&info.config);
            map.insert("config".to_string(), config_value);
            arr.push(Value::Map(map));
        }
        Value::Array(arr)
    }

    /// Convert a MountConfig to Value
    fn config_to_value(config: &MountConfig) -> Value {
        config_to_value(config)
    }
}

/// Convert a MountConfig to Value
fn config_to_value(config: &MountConfig) -> Value {
    let mut map = BTreeMap::new();
    match config {
        MountConfig::Memory => {
            map.insert("type".to_string(), Value::String("memory".to_string()));
        }
        MountConfig::Local { path } => {
            map.insert("type".to_string(), Value::String("local".to_string()));
            map.insert("path".to_string(), Value::String(path.clone()));
        }
        MountConfig::Http { url } => {
            map.insert("type".to_string(), Value::String("http".to_string()));
            map.insert("url".to_string(), Value::String(url.clone()));
        }
        MountConfig::HttpBroker => {
            map.insert("type".to_string(), Value::String("httpbroker".to_string()));
        }
        MountConfig::AsyncHttpBroker => {
            map.insert(
                "type".to_string(),
                Value::String("asynchttpbroker".to_string()),
            );
        }
        MountConfig::Structfs { url } => {
            map.insert("type".to_string(), Value::String("structfs".to_string()));
            map.insert("url".to_string(), Value::String(url.clone()));
        }
        MountConfig::Help => {
            map.insert("type".to_string(), Value::String("help".to_string()));
        }
        MountConfig::Sys => {
            map.insert("type".to_string(), Value::String("sys".to_string()));
        }
    }
    Value::Map(map)
}

/// Try to parse a Value as MountConfig
fn value_to_config(value: &Value) -> Result<MountConfig, Error> {
    match value {
        Value::Map(map) => {
            let type_str = map
                .get("type")
                .and_then(|v| match v {
                    Value::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .ok_or_else(|| Error::Decode {
                    format: crate::Format::VALUE,
                    message: "Missing 'type' field in mount config".to_string(),
                })?;

            match type_str {
                "memory" => Ok(MountConfig::Memory),
                "local" => {
                    let path = map
                        .get("path")
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .ok_or_else(|| Error::Decode {
                            format: crate::Format::VALUE,
                            message: "Missing 'path' field for local mount".to_string(),
                        })?;
                    Ok(MountConfig::Local { path })
                }
                "http" => {
                    let url = map
                        .get("url")
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .ok_or_else(|| Error::Decode {
                            format: crate::Format::VALUE,
                            message: "Missing 'url' field for http mount".to_string(),
                        })?;
                    Ok(MountConfig::Http { url })
                }
                "httpbroker" => Ok(MountConfig::HttpBroker),
                "asynchttpbroker" => Ok(MountConfig::AsyncHttpBroker),
                "structfs" => {
                    let url = map
                        .get("url")
                        .and_then(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            _ => None,
                        })
                        .ok_or_else(|| Error::Decode {
                            format: crate::Format::VALUE,
                            message: "Missing 'url' field for structfs mount".to_string(),
                        })?;
                    Ok(MountConfig::Structfs { url })
                }
                "help" => Ok(MountConfig::Help),
                "sys" => Ok(MountConfig::Sys),
                other => Err(Error::Decode {
                    format: crate::Format::VALUE,
                    message: format!("Unknown mount type: {}", other),
                }),
            }
        }
        _ => Err(Error::Decode {
            format: crate::Format::VALUE,
            message: "Mount config must be a map".to_string(),
        }),
    }
}

impl<F: StoreFactory> Reader for MountStore<F> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if Self::is_mounts_path(from) {
            // Handle reads to /ctx/mounts/*
            if from.components.len() == 2 {
                // Reading /ctx/mounts - return list of mounts
                let value = self.mounts_to_value();
                return Ok(Some(Record::parsed(value)));
            } else if let Some(name) = Self::get_mount_name(from) {
                // Reading /ctx/mounts/<name> - return mount config
                if let Some(config) = self.mounts.get(&name) {
                    let value = Self::config_to_value(config);
                    return Ok(Some(Record::parsed(value)));
                } else {
                    return Ok(None);
                }
            }
        }

        // Delegate to overlay
        self.overlay.read(from)
    }
}

impl<F: StoreFactory> Writer for MountStore<F> {
    fn write(&mut self, destination: &Path, data: Record) -> Result<Path, Error> {
        if Self::is_mounts_path(destination) {
            // Handle writes to /ctx/mounts/*
            if let Some(name) = Self::get_mount_name(destination) {
                // Get the value from the record
                let value = data.into_value(&crate::NoCodec)?;

                if value == Value::Null {
                    // Unmount
                    self.unmount(&name)?;
                } else {
                    // Parse as MountConfig and mount
                    let config = value_to_config(&value)?;
                    self.mount(&name, config)?;
                }
                return Ok(destination.clone());
            } else {
                return Err(Error::Other {
                    message: "Cannot write directly to /ctx/mounts".to_string(),
                });
            }
        }

        // Delegate to overlay
        self.overlay.write(destination, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, NoCodec};
    use std::collections::HashMap;

    // Simple test store
    struct TestStore {
        data: HashMap<Path, Record>,
    }

    impl TestStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Reader for TestStore {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl Writer for TestStore {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    // Simple factory that always creates test stores
    struct TestFactory;

    impl StoreFactory for TestFactory {
        fn create(&self, _config: &MountConfig) -> Result<StoreBox, Error> {
            Ok(Box::new(TestStore::new()))
        }
    }

    #[test]
    fn mount_and_access() {
        let mut store = MountStore::new(TestFactory);

        // Mount a store
        store.mount("data", MountConfig::Memory).unwrap();

        // Write to it
        store
            .write(&path!("data/test"), Record::parsed(Value::from("hello")))
            .unwrap();

        // Read back
        let record = store.read(&path!("data/test")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::from("hello"));
    }

    #[test]
    fn list_mounts() {
        let mut store = MountStore::new(TestFactory);

        store.mount("data", MountConfig::Memory).unwrap();
        store
            .mount(
                "local",
                MountConfig::Local {
                    path: "/tmp".to_string(),
                },
            )
            .unwrap();

        // Read /ctx/mounts
        let record = store.read(&path!("ctx/mounts")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn mount_via_write() {
        let mut store = MountStore::new(TestFactory);

        // Mount via write to /ctx/mounts/<name>
        let config = config_to_value(&MountConfig::Memory);
        store
            .write(&path!("ctx/mounts/data"), Record::parsed(config))
            .unwrap();

        // Verify mount exists
        let mounts = store.list_mounts();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].path, "data");
    }

    #[test]
    fn unmount_via_write_null() {
        let mut store = MountStore::new(TestFactory);

        // Mount first
        store.mount("data", MountConfig::Memory).unwrap();
        assert_eq!(store.list_mounts().len(), 1);

        // Unmount via write null
        store
            .write(&path!("ctx/mounts/data"), Record::parsed(Value::Null))
            .unwrap();

        assert_eq!(store.list_mounts().len(), 0);
    }

    #[test]
    fn config_conversion_roundtrip() {
        let configs = vec![
            MountConfig::Memory,
            MountConfig::Local {
                path: "/tmp/test".to_string(),
            },
            MountConfig::Http {
                url: "https://api.example.com".to_string(),
            },
            MountConfig::HttpBroker,
            MountConfig::AsyncHttpBroker,
            MountConfig::Structfs {
                url: "https://fs.example.com".to_string(),
            },
            MountConfig::Help,
            MountConfig::Sys,
        ];

        for config in configs {
            let value = config_to_value(&config);
            let back = value_to_config(&value).unwrap();
            assert_eq!(config, back);
        }
    }
}
