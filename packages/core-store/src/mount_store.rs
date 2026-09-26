//! A store that manages mounts through read/write operations.
//!
//! This store exposes mount management through the StructFS interface itself:
//! - Read `/ctx/mounts` to list all mounts
//! - Write to `/ctx/mounts/<name>` to create a mount at `/<name>`
//! - Write `null` to `/ctx/mounts/<name>` to unmount
//!
//! `MountStore` owns only the mechanism. What a mount configuration looks
//! like, and which stores it can create, belongs to the [`StoreFactory`]:
//! the factory names its config type, decodes it from the `Value` written
//! to `ctx/mounts/<name>`, encodes it back for the listing, and builds the
//! store. The REPL's factory, for example, accepts maps such as
//! `{"type": "memory"}`.
//!
//! Every mount also registers a read-only redirect from `ctx/help/<name>`
//! to `<name>/docs` (the docs protocol), so `read /ctx/help/<name>` reaches
//! the store's own documentation. Mounting never reads the store: whether
//! docs exist is discovered by the first read through the redirect.

use collection_literals::btree;
use std::collections::BTreeMap;

use crate::overlay_store::{OverlayStore, RedirectMode, StoreBox};
use crate::{path, Error, Path, Reader, Record, Value, Writer};

/// Creates stores from mount configurations and defines their wire form.
///
/// `MountStore` stores configs as `Self::Config` and never interprets them;
/// it converts through [`config_from_value`](Self::config_from_value) when a
/// config is written to `ctx/mounts/<name>` and through
/// [`config_to_value`](Self::config_to_value) when the listing is read.
pub trait StoreFactory: Send + Sync {
    /// The factory's mount configuration.
    type Config: Clone + Send + Sync;

    /// Build the store a config describes.
    fn create(&self, config: &Self::Config) -> Result<StoreBox, Error>;

    /// Decode a config from the value written to `ctx/mounts/<name>`.
    /// Malformed configs should be `Error::InvalidArgument`.
    fn config_from_value(&self, value: Value) -> Result<Self::Config, Error>;

    /// Encode a config for the `ctx/mounts` listing.
    fn config_to_value(&self, config: &Self::Config) -> Result<Value, Error>;
}

/// A store that manages mounts through read/write operations.
///
/// Every mount name is registered exactly once: mounting a name that is
/// already taken is `Error::Conflict` (unmount first), and `unmount` works
/// for names mounted through the factory and for pre-built stores mounted
/// with [`mount_store`](Self::mount_store) alike. Stores mounted without a
/// config list with a `null` config.
pub struct MountStore<F: StoreFactory> {
    overlay: OverlayStore,
    /// Registered mounts; `None` for stores mounted without a config.
    mounts: BTreeMap<String, Option<F::Config>>,
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

    /// Mount a store created by the factory from `config` at `name`.
    ///
    /// Fails with `Error::Conflict` if `name` is already mounted; the
    /// factory is not consulted in that case.
    pub fn mount(&mut self, name: &str, config: F::Config) -> Result<(), Error> {
        let mount_path = self.reserve(name)?;
        let store = self.factory.create(&config)?;
        self.install(mount_path, store, Some(config));
        Ok(())
    }

    /// Mount a pre-created store at the given path.
    ///
    /// This bypasses the factory and allows mounting stores that have
    /// complex initialization requirements (e.g., cross-store dependencies).
    /// The mount is tracked like any other (it can be unmounted and lists
    /// with a `null` config). Fails with `Error::Conflict` if `name` is
    /// already mounted.
    pub fn mount_store(&mut self, name: &str, store: StoreBox) -> Result<(), Error> {
        let mount_path = self.reserve(name)?;
        self.install(mount_path, store, None);
        Ok(())
    }

    /// Validate `name` as a mount path and check it is free. Names are
    /// compared as normalized paths, so `a/` and `a` are the same mount.
    fn reserve(&self, name: &str) -> Result<Path, Error> {
        let mount_path = Path::parse(name)?;
        let key = mount_path.to_string();
        if self.mounts.contains_key(&key) {
            return Err(Error::conflict(format!("'{key}' is already mounted")));
        }
        Ok(mount_path)
    }

    /// Route the store, record the mount, and link its docs into `ctx/help`.
    fn install(&mut self, mount_path: Path, store: StoreBox, config: Option<F::Config>) {
        let name = mount_path.to_string();
        self.overlay.mount_boxed(mount_path.clone(), store);
        self.mounts.insert(name.clone(), config);

        // `ctx/help/<name>` -> `<name>/docs`. Registered without reading the
        // store: a mount must not perform operations on the store it mounts.
        // (Mounting at `ctx` or `ctx/help` itself would alias the help tree
        // onto itself, so those names get no redirect.)
        let help_prefix = path!("ctx/help");
        if !help_prefix.has_prefix(&mount_path) {
            self.overlay.add_redirect(
                help_prefix.join(&mount_path),
                mount_path.join(&path!("docs")),
                RedirectMode::ReadOnly,
                Some(name),
            );
        }
    }

    /// Unmount the store at `name`, along with any redirects it created.
    ///
    /// Fails with `Error::NotFound` if nothing is mounted there.
    pub fn unmount(&mut self, name: &str) -> Result<(), Error> {
        let mount_path = Path::parse(name)?;
        let key = mount_path.to_string();
        if !self.mounts.contains_key(&key) {
            return Err(Error::not_found(mount_path));
        }

        // Remove from overlay (the actual routing)
        self.overlay.unmount(&mount_path);

        // Cascade: remove any redirects this mount created
        self.overlay.remove_redirects_for_mount(&key);

        // Remove from tracking (the metadata)
        self.mounts.remove(&key);

        Ok(())
    }

    /// List all redirects in the overlay.
    pub fn list_redirects(&self) -> Vec<(Path, Path, RedirectMode)> {
        self.overlay.list_redirects()
    }

    /// List all mounts as `(name, config)`; the config is `None` for stores
    /// mounted with [`mount_store`](Self::mount_store).
    pub fn list_mounts(&self) -> Vec<(String, Option<F::Config>)> {
        self.mounts
            .iter()
            .map(|(path, config)| (path.clone(), config.clone()))
            .collect()
    }

    /// The factory that builds this store's mounts.
    pub fn factory(&self) -> &F {
        &self.factory
    }

    fn encode(&self, config: Option<&F::Config>) -> Result<Value, Error> {
        config.map_or(Ok(Value::Null), |config| {
            self.factory.config_to_value(config)
        })
    }

    fn is_mounts_path(path: &Path) -> bool {
        path.len() >= 2 && &path[0] == MOUNTS_PREFIX[0] && &path[1] == MOUNTS_PREFIX[1]
    }

    fn get_mount_name(path: &Path) -> Option<String> {
        if path.len() >= 3 && &path[0] == MOUNTS_PREFIX[0] && &path[1] == MOUNTS_PREFIX[1] {
            Some(path.slice(2, path.len()).to_string())
        } else {
            None
        }
    }

    /// The mount listing as a Value: `[{"path": name, "config": {...}|null}]`.
    fn mounts_to_value(&self) -> Result<Value, Error> {
        let arr = self
            .mounts
            .iter()
            .map(|(path, config)| {
                Ok(Value::Map(btree! {
                    "path".to_string() => Value::String(path.clone()),
                    "config".to_string() => self.encode(config.as_ref())?,
                }))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Value::Array(arr))
    }

    /// The value served at `ctx/mounts...`, if `from` addresses that tree.
    fn read_mounts_value(&self, from: &Path) -> Option<Result<Option<Value>, Error>> {
        if !Self::is_mounts_path(from) {
            return None;
        }
        if from.len() == 2 {
            return Some(self.mounts_to_value().map(Some));
        }
        let name = Self::get_mount_name(from)?;
        Some(
            self.mounts
                .get(&name)
                .map(|config| self.encode(config.as_ref()))
                .transpose(),
        )
    }

    /// Child names under `ctx/mounts...`, if `from` addresses that tree.
    /// The listing's children are the mount names (which may contain `/`),
    /// not array indices; a config's children are its keys.
    fn mounts_children(&self, from: &Path) -> Option<Result<Option<Vec<String>>, Error>> {
        if Self::is_mounts_path(from) && from.len() == 2 {
            return Some(Ok(Some(self.mounts.keys().cloned().collect())));
        }
        self.read_mounts_value(from)
            .map(|value| value.map(|value| value.as_ref().map(crate::children::names_of_value)))
    }
}

impl<F: StoreFactory> Reader for MountStore<F> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if let Some(value) = self.read_mounts_value(from) {
            return Ok(value?.map(Record::parsed));
        }
        self.overlay.read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        if let Some(names) = self.mounts_children(from) {
            return names;
        }
        self.overlay.read_children(from)
    }

    fn read_children_page(
        &mut self,
        from: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Option<crate::ChildPage>, Error> {
        if let Some(names) = self.mounts_children(from) {
            if limit == 0 {
                return Err(Error::invalid_argument("child page limit must be positive"));
            }
            return names?
                .map(|names| crate::children::page_names(names, offset, limit))
                .transpose();
        }
        self.overlay.read_children_page(from, offset, limit)
    }
}

impl<F: StoreFactory> Writer for MountStore<F> {
    fn write(&mut self, destination: &Path, data: Record) -> Result<Path, Error> {
        if Self::is_mounts_path(destination) {
            // Handle writes to /ctx/mounts/*
            let Some(name) = Self::get_mount_name(destination) else {
                return Err(Error::permission_denied(
                    "cannot write directly to ctx/mounts; write a config to ctx/mounts/<name>",
                ));
            };
            // Get the value from the record
            let value = data.into_value(&crate::NoCodec)?;

            if value == Value::Null {
                // Unmount
                self.unmount(&name)?;
            } else {
                let config = self.factory.config_from_value(value)?;
                self.mount(&name, config)?;
            }
            return Ok(destination.clone());
        }

        // Delegate to overlay
        self.overlay.write(destination, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, MemoryStore, NoCodec};

    /// A config the tests' factories understand: any map with a string
    /// `type`. The mechanism never looks inside it.
    #[derive(Debug, Clone, PartialEq)]
    struct TestConfig(Value);

    fn memory() -> TestConfig {
        TestConfig(Value::Map(
            btree! {"type".to_string() => Value::from("memory")},
        ))
    }

    fn local(path: &str) -> TestConfig {
        TestConfig(Value::Map(btree! {
            "type".to_string() => Value::from("local"),
            "path".to_string() => Value::from(path),
        }))
    }

    fn decode_test_config(value: Value) -> Result<TestConfig, Error> {
        match &value {
            Value::Map(map) if matches!(map.get("type"), Some(Value::String(_))) => {
                Ok(TestConfig(value))
            }
            _ => Err(Error::invalid_argument("config needs a string 'type'")),
        }
    }

    /// Implements the config half of `StoreFactory` for `TestConfig`.
    macro_rules! test_config_codec {
        () => {
            type Config = TestConfig;
            fn config_from_value(&self, value: Value) -> Result<TestConfig, Error> {
                decode_test_config(value)
            }
            fn config_to_value(&self, config: &TestConfig) -> Result<Value, Error> {
                Ok(config.0.clone())
            }
        };
    }

    // Simple factory that always creates memory stores
    struct TestFactory;

    impl StoreFactory for TestFactory {
        test_config_codec!();
        fn create(&self, _config: &TestConfig) -> Result<StoreBox, Error> {
            Ok(Box::new(MemoryStore::new()))
        }
    }

    #[test]
    fn mount_and_access() {
        let mut store = MountStore::new(TestFactory);

        // Mount a store
        store.mount("data", memory()).unwrap();

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

        store.mount("data", memory()).unwrap();
        store.mount("local", local("/tmp")).unwrap();

        // Read /ctx/mounts
        let record = store.read(&path!("ctx/mounts")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
            }
            _ => panic!("expected array"),
        }
        assert_eq!(
            store.list_mounts(),
            vec![
                ("data".to_string(), Some(memory())),
                ("local".to_string(), Some(local("/tmp"))),
            ]
        );
    }

    #[test]
    fn mount_via_write() {
        let mut store = MountStore::new(TestFactory);

        // Mount via write to /ctx/mounts/<name>
        store
            .write(&path!("ctx/mounts/data"), Record::parsed(memory().0))
            .unwrap();

        // Verify mount exists, and the listing is the factory's encoding.
        let mounts = store.list_mounts();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].0, "data");
        assert_eq!(
            store
                .read(&path!("ctx/mounts/data"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&memory().0)
        );
    }

    #[test]
    fn malformed_config_is_the_factorys_error_and_mounts_nothing() {
        let mut store = MountStore::new(TestFactory);
        for bad in [
            Value::from("not a map"),
            Value::Map(BTreeMap::new()),
            Value::Map(btree! {"type".to_string() => Value::Integer(1)}),
        ] {
            let err = store
                .write(&path!("ctx/mounts/data"), Record::parsed(bad))
                .unwrap_err();
            assert!(matches!(err, Error::InvalidArgument { .. }), "{err}");
        }
        assert!(store.list_mounts().is_empty());
    }

    #[test]
    fn listing_propagates_encode_errors() {
        struct Opaque;
        impl StoreFactory for Opaque {
            type Config = ();
            fn create(&self, _: &()) -> Result<StoreBox, Error> {
                Ok(Box::new(MemoryStore::new()))
            }
            fn config_from_value(&self, _: Value) -> Result<(), Error> {
                Ok(())
            }
            fn config_to_value(&self, _: &()) -> Result<Value, Error> {
                Err(Error::invalid_argument("unencodable"))
            }
        }
        let mut store = MountStore::new(Opaque);
        store.mount("x", ()).unwrap();
        assert!(matches!(
            store.read(&path!("ctx/mounts")),
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            store.read_children(&path!("ctx/mounts/x")),
            Err(Error::InvalidArgument { .. })
        ));
        // Unrelated reads are unaffected.
        assert!(store.read(&path!("x/anything")).unwrap().is_none());
    }

    #[test]
    fn unmount_via_write_null() {
        let mut store = MountStore::new(TestFactory);

        // Mount first
        store.mount("data", memory()).unwrap();
        assert_eq!(store.list_mounts().len(), 1);

        // Unmount via write null
        store
            .write(&path!("ctx/mounts/data"), Record::parsed(Value::Null))
            .unwrap();

        assert_eq!(store.list_mounts().len(), 0);
    }

    #[test]
    fn mount_store_directly() {
        let mut store = MountStore::new(TestFactory);

        // Mount a store directly without using factory
        store
            .mount_store("direct", Box::new(MemoryStore::new()))
            .unwrap();

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

        // Tracked like any other mount: listed with a null config, and
        // unmountable.
        assert_eq!(store.list_mounts(), vec![("direct".to_string(), None)]);
        assert_eq!(
            store
                .read(&path!("ctx/mounts/direct"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::Null)
        );
        store.unmount("direct").unwrap();
        assert!(store.list_mounts().is_empty());
        assert!(matches!(
            store.read(&path!("direct/test")),
            Err(Error::NoRoute { .. })
        ));
    }

    #[test]
    fn duplicate_mount_name_is_conflict() {
        struct CountingFactory(std::sync::Mutex<usize>);
        impl StoreFactory for CountingFactory {
            test_config_codec!();
            fn create(&self, _config: &TestConfig) -> Result<StoreBox, Error> {
                *self.0.lock().unwrap() += 1;
                Ok(Box::new(MemoryStore::new()))
            }
        }

        let mut store = MountStore::new(CountingFactory(std::sync::Mutex::new(0)));
        store.mount("data", memory()).unwrap();
        store
            .write(&path!("data/key"), Record::parsed(Value::Integer(1)))
            .unwrap();

        let err = store.mount("data", memory()).unwrap_err();
        assert!(matches!(err, Error::Conflict { .. }));
        // The factory was not consulted for the rejected mount, and the
        // original store is untouched.
        assert_eq!(*store.factory.0.lock().unwrap(), 1);
        assert!(store.read(&path!("data/key")).unwrap().is_some());

        assert!(matches!(
            store.mount_store("data", Box::new(MemoryStore::new())),
            Err(Error::Conflict { .. })
        ));
        assert!(matches!(
            store.write(&path!("ctx/mounts/data"), Record::parsed(memory().0)),
            Err(Error::Conflict { .. })
        ));
    }

    #[test]
    fn mount_does_not_read_the_mounted_store() {
        /// A store that fails loudly if anything reads it.
        struct Touchy;
        impl Reader for Touchy {
            fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
                panic!("mount must not read the store (read {from})")
            }
        }
        impl Writer for Touchy {
            fn write(&mut self, to: &Path, _: Record) -> Result<Path, Error> {
                Ok(to.clone())
            }
        }
        struct TouchyFactory;
        impl StoreFactory for TouchyFactory {
            test_config_codec!();
            fn create(&self, _: &TestConfig) -> Result<StoreBox, Error> {
                Ok(Box::new(Touchy))
            }
        }

        let mut store = MountStore::new(TouchyFactory);
        store.mount("quiet", memory()).unwrap();
        store.mount_store("quieter", Box::new(Touchy)).unwrap();

        // The docs redirect exists without having probed the store.
        let redirects = store.list_redirects();
        assert!(redirects.contains(&(
            path!("ctx/help/quiet"),
            path!("quiet/docs"),
            RedirectMode::ReadOnly
        )));
        assert!(redirects.contains(&(
            path!("ctx/help/quieter"),
            path!("quieter/docs"),
            RedirectMode::ReadOnly
        )));
        // Unmount cascades the redirect away.
        store.unmount("quiet").unwrap();
        assert_eq!(store.list_redirects().len(), 1);
    }

    #[test]
    fn help_redirect_serves_docs_when_present() {
        let mut store = MountStore::new(TestFactory);
        store.mount("documented", memory()).unwrap();
        store.mount("bare", memory()).unwrap();
        store
            .write(
                &path!("documented/docs"),
                Record::parsed(Value::from("manual")),
            )
            .unwrap();

        assert_eq!(
            store
                .read(&path!("ctx/help/documented"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("manual"))
        );
        // A store without docs reads as absent, not as an error.
        assert!(store.read(&path!("ctx/help/bare")).unwrap().is_none());
        // Writes through the help redirect are denied.
        assert!(matches!(
            store.write(&path!("ctx/help/bare"), Record::parsed(Value::Null)),
            Err(Error::PermissionDenied { .. })
        ));
    }

    #[test]
    fn mounting_the_help_tree_itself_adds_no_self_redirect() {
        let mut store = MountStore::new(TestFactory);
        store
            .mount_store("ctx/help", Box::new(MemoryStore::new()))
            .unwrap();
        store
            .mount_store("ctx", Box::new(MemoryStore::new()))
            .unwrap();
        assert!(store.list_redirects().is_empty());
        store.unmount("ctx/help").unwrap();
        store.unmount("ctx").unwrap();
    }

    #[test]
    fn unmount_nonexistent_is_not_found() {
        let mut store = MountStore::new(TestFactory);

        let result = store.unmount("nonexistent");
        assert!(matches!(
            result,
            Err(Error::NotFound { path }) if path == path!("nonexistent")
        ));
    }

    #[test]
    fn read_specific_mount_config() {
        let mut store = MountStore::new(TestFactory);

        store.mount("mydata", local("/my/path")).unwrap();

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
    fn write_directly_to_mounts_is_permission_denied() {
        let mut store = MountStore::new(TestFactory);

        // Try to write directly to /ctx/mounts (without specifying a name)
        let result = store.write(&path!("ctx/mounts"), Record::parsed(Value::Null));
        assert!(matches!(result, Err(Error::PermissionDenied { .. })));
    }

    // Factory that fails
    struct FailingFactory;

    impl StoreFactory for FailingFactory {
        test_config_codec!();
        fn create(&self, _config: &TestConfig) -> Result<StoreBox, Error> {
            Err(Error::store("factory", "create", "Factory failed"))
        }
    }

    #[test]
    fn mount_with_failing_factory() {
        let mut store = MountStore::new(FailingFactory);

        let result = store.mount("data", memory());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Factory failed"));
        // A failed mount leaves no registration behind.
        assert!(store.list_mounts().is_empty());
        assert!(store.list_redirects().is_empty());
    }

    #[test]
    fn nested_mount_path() {
        let mut store = MountStore::new(TestFactory);

        // Mount via write to a nested path: /ctx/mounts/nested/path
        store
            .write(&path!("ctx/mounts/nested/path"), Record::parsed(memory().0))
            .unwrap();

        // Verify mount exists with nested name
        let mounts = store.list_mounts();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].0, "nested/path");
    }

    #[test]
    fn delegate_to_overlay_read() {
        let mut store = MountStore::new(TestFactory);

        // Read from unmounted path (delegates to empty overlay)
        // Overlay returns an error when no route is found
        let result = store.read(&path!("unmounted/path"));
        assert!(matches!(result, Err(Error::NoRoute { .. })));
    }

    #[test]
    fn discovery_verbs_route_to_mounted_store() {
        let mut store = MountStore::new(TestFactory);
        store.mount("data", memory()).unwrap();
        store
            .write(
                &path!("data/users/alice"),
                Record::parsed(Value::Integer(1)),
            )
            .unwrap();
        store
            .write(&path!("data/users/bob"), Record::parsed(Value::Integer(2)))
            .unwrap();

        assert_eq!(
            store.read_children(&path!("data/users")).unwrap(),
            Some(vec!["alice".to_string(), "bob".to_string()])
        );
        let page = store
            .read_children_page(&path!("data/users"), 1, 5)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["bob".to_string()]);
        assert_eq!(page.next, None);
        assert_eq!(store.read_children(&path!("data/missing")).unwrap(), None);
        assert!(matches!(
            store.read_children(&path!("unmounted")),
            Err(Error::NoRoute { .. })
        ));
    }

    #[test]
    fn discovery_verbs_cover_the_mount_listing() {
        let mut store = MountStore::new(TestFactory);
        store.mount("a", memory()).unwrap();
        store.mount("b", local("/x")).unwrap();

        store
            .mount_store("nested/name", Box::new(MemoryStore::new()))
            .unwrap();

        // The listing's children are mount names, not array indices.
        assert_eq!(
            store.read_children(&path!("ctx/mounts")).unwrap(),
            Some(vec![
                "a".to_string(),
                "b".to_string(),
                "nested/name".to_string()
            ])
        );
        let page = store
            .read_children_page(&path!("ctx/mounts"), 1, 1)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["b".to_string()]);
        // A config map: children are its keys.
        assert_eq!(
            store.read_children(&path!("ctx/mounts/b")).unwrap(),
            Some(vec!["path".to_string(), "type".to_string()])
        );
        assert_eq!(
            store.read_children(&path!("ctx/mounts/missing")).unwrap(),
            None
        );
        assert!(matches!(
            store.read_children_page(&path!("ctx/mounts"), 0, 0),
            Err(Error::InvalidArgument { .. })
        ));
    }

    #[test]
    fn mount_names_are_normalized_paths() {
        let mut store = MountStore::new(TestFactory);
        store.mount("a/", memory()).unwrap();
        assert!(matches!(
            store.mount("a", memory()),
            Err(Error::Conflict { .. })
        ));
        assert!(matches!(
            store.mount_store("/a", Box::new(MemoryStore::new())),
            Err(Error::Conflict { .. })
        ));
        assert_eq!(store.list_mounts(), vec![("a".to_string(), Some(memory()))]);
        assert!(store.read(&path!("ctx/mounts/a")).unwrap().is_some());
        // Unmount by any spelling of the same path, redirect included.
        store.unmount("a/").unwrap();
        assert!(store.list_mounts().is_empty());
        assert!(store.list_redirects().is_empty());
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
        store.mount("data", memory()).unwrap();

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
        store.mount("data", memory()).unwrap();
        store
            .write(&path!("data/key"), Record::parsed(Value::Integer(1)))
            .unwrap();

        store.unmount("data").unwrap();
        store.mount("data", memory()).unwrap();

        // New mount should be empty
        let result = store.read(&path!("data/key")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn unmount_priority_preserved() {
        let mut store = MountStore::new(TestFactory);

        // Mount two stores at overlapping paths
        store.mount("data", memory()).unwrap();
        store.mount("data/nested", memory()).unwrap();

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
