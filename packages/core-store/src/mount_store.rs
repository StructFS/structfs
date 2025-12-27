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

use crate::overlay_store::{OverlayStore, RedirectMode, StoreBox};
use crate::{path, Error, Path, Reader, Record, Value, Writer};

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
    /// REPL documentation store
    Repl,
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
        self.overlay.mount(mount_path.clone(), store);

        // Track the mount
        self.mounts.insert(name.to_string(), config);

        // Discover and redirect docs
        self.discover_and_redirect_docs(name, &mount_path);

        Ok(())
    }

    /// Probe for docs at mount path and create redirect if found.
    fn discover_and_redirect_docs(&mut self, name: &str, mount_path: &Path) {
        let docs_path = mount_path.join(&path!("docs"));

        // Probe for docs - if readable, create redirect
        if self.overlay.read(&docs_path).ok().flatten().is_some() {
            // Use the full mount path for the help path
            // e.g., "ctx/sys" -> help path is "ctx/help/ctx/sys"
            // This allows `read /ctx/help/ctx/sys` to get docs for the store at `/ctx/sys`
            if let Ok(help_suffix) = Path::parse(name) {
                let help_path = path!("ctx/help").join(&help_suffix);

                self.overlay.add_redirect(
                    help_path,
                    docs_path,
                    RedirectMode::ReadOnly,
                    Some(name.to_string()),
                );
            }
        }
    }

    /// Mount a pre-created store at the given path.
    ///
    /// This bypasses the factory and allows mounting stores that have
    /// complex initialization requirements (e.g., cross-store dependencies).
    pub fn mount_store(&mut self, name: &str, store: StoreBox) -> Result<(), Error> {
        // Parse the mount path
        let mount_path = Path::parse(name).map_err(Error::Path)?;

        // Add to overlay
        self.overlay.mount(mount_path.clone(), store);

        // Discover and redirect docs
        self.discover_and_redirect_docs(name, &mount_path);

        // Don't track in mounts BTreeMap since we don't have a config
        // This mount won't show up in list_mounts or be serializable,
        // which is fine for built-in stores like help

        Ok(())
    }

    /// Unmount a store at the given path
    pub fn unmount(&mut self, name: &str) -> Result<(), Error> {
        if !self.mounts.contains_key(name) {
            return Err(Error::store(
                "mount_store",
                "unmount",
                format!("No mount at '{}'", name),
            ));
        }

        // Parse the mount path
        let mount_path = Path::parse(name)?;

        // Remove from overlay (the actual routing)
        self.overlay.unmount(&mount_path);

        // Cascade: remove any redirects this mount created
        self.overlay.remove_redirects_for_mount(name);

        // Remove from tracking (the metadata)
        self.mounts.remove(name);

        Ok(())
    }

    /// List all redirects in the overlay.
    pub fn list_redirects(&self) -> Vec<(Path, Path, RedirectMode)> {
        self.overlay.list_redirects()
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
        MountConfig::Repl => {
            map.insert("type".to_string(), Value::String("repl".to_string()));
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
                .ok_or_else(|| {
                    Error::decode(crate::Format::VALUE, "Missing 'type' field in mount config")
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
                        .ok_or_else(|| {
                            Error::decode(
                                crate::Format::VALUE,
                                "Missing 'path' field for local mount",
                            )
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
                        .ok_or_else(|| {
                            Error::decode(
                                crate::Format::VALUE,
                                "Missing 'url' field for http mount",
                            )
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
                        .ok_or_else(|| {
                            Error::decode(
                                crate::Format::VALUE,
                                "Missing 'url' field for structfs mount",
                            )
                        })?;
                    Ok(MountConfig::Structfs { url })
                }
                "help" => Ok(MountConfig::Help),
                "sys" => Ok(MountConfig::Sys),
                "repl" => Ok(MountConfig::Repl),
                other => Err(Error::decode(
                    crate::Format::VALUE,
                    format!("Unknown mount type: {}", other),
                )),
            }
        }
        _ => Err(Error::decode(
            crate::Format::VALUE,
            "Mount config must be a map",
        )),
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
                return Err(Error::store(
                    "mount_store",
                    "write",
                    "Cannot write directly to /ctx/mounts",
                ));
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
            MountConfig::Repl,
        ];

        for config in configs {
            let value = config_to_value(&config);
            let back = value_to_config(&value).unwrap();
            assert_eq!(config, back);
        }
    }

    #[test]
    fn mount_store_directly() {
        let mut store = MountStore::new(TestFactory);

        // Mount a store directly without using factory
        let test_store = Box::new(TestStore::new());
        store.mount_store("direct", test_store).unwrap();

        // Write to it
        store
            .write(
                &path!("direct/test"),
                Record::parsed(Value::from("direct_value")),
            )
            .unwrap();

        // Read back
        let record = store.read(&path!("direct/test")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::from("direct_value"));
    }

    #[test]
    fn unmount_nonexistent_fails() {
        let mut store = MountStore::new(TestFactory);

        let result = store.unmount("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No mount"));
    }

    #[test]
    fn read_specific_mount_config() {
        let mut store = MountStore::new(TestFactory);

        store
            .mount(
                "mydata",
                MountConfig::Local {
                    path: "/my/path".to_string(),
                },
            )
            .unwrap();

        // Read /ctx/mounts/mydata
        let record = store.read(&path!("ctx/mounts/mydata")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("type"), Some(&Value::String("local".to_string())));
                assert_eq!(
                    map.get("path"),
                    Some(&Value::String("/my/path".to_string()))
                );
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn read_nonexistent_mount_config() {
        let mut store = MountStore::new(TestFactory);

        // Read /ctx/mounts/nonexistent
        let result = store.read(&path!("ctx/mounts/nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn write_directly_to_mounts_fails() {
        let mut store = MountStore::new(TestFactory);

        // Try to write directly to /ctx/mounts (without specifying a name)
        let result = store.write(&path!("ctx/mounts"), Record::parsed(Value::Null));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot write"));
    }

    #[test]
    fn value_to_config_unknown_type_fails() {
        let mut map = BTreeMap::new();
        map.insert("type".to_string(), Value::String("unknown".to_string()));
        let result = value_to_config(&Value::Map(map));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unknown mount type"));
    }

    #[test]
    fn value_to_config_non_map_fails() {
        let result = value_to_config(&Value::String("not a map".to_string()));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be a map"));
    }

    #[test]
    fn value_to_config_missing_type_fails() {
        let map = BTreeMap::new();
        let result = value_to_config(&Value::Map(map));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'type'"));
    }

    #[test]
    fn value_to_config_type_not_string_fails() {
        let mut map = BTreeMap::new();
        map.insert("type".to_string(), Value::Integer(123));
        let result = value_to_config(&Value::Map(map));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'type'"));
    }

    #[test]
    fn value_to_config_local_missing_path_fails() {
        let mut map = BTreeMap::new();
        map.insert("type".to_string(), Value::String("local".to_string()));
        let result = value_to_config(&Value::Map(map));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'path'"));
    }

    #[test]
    fn value_to_config_http_missing_url_fails() {
        let mut map = BTreeMap::new();
        map.insert("type".to_string(), Value::String("http".to_string()));
        let result = value_to_config(&Value::Map(map));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'url'"));
    }

    #[test]
    fn value_to_config_structfs_missing_url_fails() {
        let mut map = BTreeMap::new();
        map.insert("type".to_string(), Value::String("structfs".to_string()));
        let result = value_to_config(&Value::Map(map));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing 'url'"));
    }

    // Factory that fails
    struct FailingFactory;

    impl StoreFactory for FailingFactory {
        fn create(&self, _config: &MountConfig) -> Result<StoreBox, Error> {
            Err(Error::store("factory", "create", "Factory failed"))
        }
    }

    #[test]
    fn mount_with_failing_factory() {
        let mut store = MountStore::new(FailingFactory);

        let result = store.mount("data", MountConfig::Memory);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Factory failed"));
    }

    #[test]
    fn mount_info_serialization() {
        let info = MountInfo {
            path: "/test".to_string(),
            config: MountConfig::Memory,
        };

        // Test Debug impl
        let debug = format!("{:?}", info);
        assert!(debug.contains("/test"));
        assert!(debug.contains("Memory"));

        // Test Clone
        let cloned = info.clone();
        assert_eq!(cloned.path, "/test");
    }

    #[test]
    fn mount_config_debug_clone() {
        // Test Debug and Clone on MountConfig
        let config = MountConfig::Http {
            url: "https://test.com".to_string(),
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("https://test.com"));

        let cloned = config.clone();
        assert_eq!(cloned, config);
    }

    #[test]
    fn nested_mount_path() {
        let mut store = MountStore::new(TestFactory);

        // Mount via write to a nested path: /ctx/mounts/nested/path
        let config = config_to_value(&MountConfig::Memory);
        store
            .write(&path!("ctx/mounts/nested/path"), Record::parsed(config))
            .unwrap();

        // Verify mount exists with nested name
        let mounts = store.list_mounts();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].path, "nested/path");
    }

    #[test]
    fn delegate_to_overlay_read() {
        let mut store = MountStore::new(TestFactory);

        // Read from unmounted path (delegates to empty overlay)
        // Overlay returns an error when no route is found
        let result = store.read(&path!("unmounted/path"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no route"));
    }

    #[test]
    fn is_mounts_path_variations() {
        // Test the is_mounts_path helper
        assert!(MountStore::<TestFactory>::is_mounts_path(&path!(
            "ctx/mounts"
        )));
        assert!(MountStore::<TestFactory>::is_mounts_path(&path!(
            "ctx/mounts/foo"
        )));
        assert!(MountStore::<TestFactory>::is_mounts_path(&path!(
            "ctx/mounts/foo/bar"
        )));
        assert!(!MountStore::<TestFactory>::is_mounts_path(&path!("ctx")));
        assert!(!MountStore::<TestFactory>::is_mounts_path(&path!(
            "ctx/other"
        )));
        assert!(!MountStore::<TestFactory>::is_mounts_path(&path!("other")));
    }

    #[test]
    fn get_mount_name_variations() {
        // Test the get_mount_name helper
        assert_eq!(
            MountStore::<TestFactory>::get_mount_name(&path!("ctx/mounts/foo")),
            Some("foo".to_string())
        );
        assert_eq!(
            MountStore::<TestFactory>::get_mount_name(&path!("ctx/mounts/foo/bar")),
            Some("foo/bar".to_string())
        );
        assert_eq!(
            MountStore::<TestFactory>::get_mount_name(&path!("ctx/mounts")),
            None
        );
        assert_eq!(
            MountStore::<TestFactory>::get_mount_name(&path!("ctx")),
            None
        );
    }

    #[test]
    fn unmount_removes_from_overlay() {
        let mut store = MountStore::new(TestFactory);
        store.mount("data", MountConfig::Memory).unwrap();

        // Write something
        store
            .write(&path!("data/key"), Record::parsed(Value::Integer(42)))
            .unwrap();

        // Verify it's readable
        let result = store.read(&path!("data/key")).unwrap();
        assert!(result.is_some());

        // Unmount
        store.unmount("data").unwrap();

        // Verify it's no longer routable (should return NoRoute error)
        let result = store.read(&path!("data/key"));
        assert!(result.is_err());
    }

    #[test]
    fn unmount_allows_remount() {
        let mut store = MountStore::new(TestFactory);
        store.mount("data", MountConfig::Memory).unwrap();
        store
            .write(&path!("data/key"), Record::parsed(Value::Integer(1)))
            .unwrap();

        store.unmount("data").unwrap();
        store.mount("data", MountConfig::Memory).unwrap();

        // New mount should be empty
        let result = store.read(&path!("data/key")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn unmount_priority_preserved() {
        let mut store = MountStore::new(TestFactory);

        // Mount two stores at overlapping paths
        store.mount("data", MountConfig::Memory).unwrap();
        store.mount("data/nested", MountConfig::Memory).unwrap();

        // Write to nested
        store
            .write(&path!("data/nested/key"), Record::parsed(Value::Integer(1)))
            .unwrap();

        // Unmount nested
        store.unmount("data/nested").unwrap();

        // data should still work
        store
            .write(&path!("data/other"), Record::parsed(Value::Integer(2)))
            .unwrap();
        let result = store.read(&path!("data/other")).unwrap();
        assert!(result.is_some());
    }
}
