//! Store context for the REPL.
//!
//! This module provides the store context that manages mounts and registers.

use collection_literals::btree;
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use structfs_core_store::{
    mount_store::{MountStore, StoreFactory},
    path, Error as CoreError, NoCodec, Path, Reader, Record, Value, Writer,
};

use crate::help_store::{HelpStore, HelpStoreHandle, HelpStoreState};
use crate::mounts::{CoreReplStoreFactory, DefaultMountKind, MountConfig, DEFAULT_MOUNTS};

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ContextError {
    #[error("Store error: {0}")]
    Store(#[from] CoreError),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Session-local named values, mounted at `/ctx/registers`.
///
/// Reading the root lists register names; `name/sub/path` reads into a
/// register's value. Writing `name` replaces a register; writing
/// `name/sub/path` sets that child inside the register's value (creating
/// the register as a map, and intermediate maps, as needed).
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

    /// Documentation for the register store.
    fn docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Registers".into()),
            "description".into() => Value::String("Named storage for command outputs".into()),
            "syntax".into() => Value::Map(btree! {
                "capture".into() => Value::String("@name <command> - Store command output in register".into()),
                "read".into() => Value::String("read @name - Read register value".into()),
                "dereference".into() => Value::String("*@name - Use register value as path".into()),
                "write".into() => Value::String("write @name <value> - Set register directly".into()),
            }),
            "examples".into() => Value::Array(vec![
                Value::String("@result read /ctx/sys/time/now".into()),
                Value::String("read @result".into()),
                Value::String("@path read /ctx/sys/env/HOME".into()),
                Value::String("read *@path".into()),
            ]),
            "keywords".into() => Value::Array(vec![
                Value::String("registers".into()),
                Value::String("variables".into()),
                Value::String("capture".into()),
                Value::String("storage".into()),
            ]),
        })
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

        // Handle docs path
        if &from[0] == "docs" {
            return Ok(Some(Record::parsed(Self::docs())));
        }

        let register_name = &from[0];
        let sub_path = from.slice(1, from.len());

        let register_value = match self.registers.get(register_name) {
            Some(v) => v,
            None => return Ok(None),
        };

        let value = match register_value.get(&sub_path) {
            Some(v) => v.clone(),
            None => return Ok(None),
        };

        Ok(Some(Record::parsed(value)))
    }

    /// The root's children are the register names (the root reads as an
    /// array of them, whose default children would be indices).
    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, CoreError> {
        if from.is_empty() {
            return Ok(Some(self.registers.keys().cloned().collect()));
        }
        Ok(self
            .read(from)?
            .and_then(|record| record.into_value(&NoCodec).ok())
            .map(|value| match value {
                Value::Map(map) => map.into_keys().collect(),
                Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
                _ => Vec::new(),
            }))
    }
}

impl Writer for RegisterStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, CoreError> {
        if to.is_empty() {
            return Err(CoreError::invalid_argument(
                "Cannot write to register root. Use @name to specify a register.",
            ));
        }

        let value = data.into_value(&NoCodec)?;
        let register_name = &to[0];
        let sub_path = to.slice(1, to.len());

        if sub_path.is_empty() {
            self.registers.insert(register_name.to_string(), value);
        } else {
            let register = self
                .registers
                .entry(register_name.to_string())
                .or_insert_with(Value::map);
            register.set(&sub_path, value)?;
        }
        Ok(to.clone())
    }
}

/// The REPL's view of the store tree: mounts, registers, and the current
/// path.
///
/// The context is generic over a `StoreFactory` implementation, allowing
/// different factories to be used for testing or alternative configurations.
/// By default, it uses `CoreReplStoreFactory` which creates all standard stores.
///
/// Registers live in the store mounted at `/ctx/registers/`; `@name` is
/// sugar for `/ctx/registers/name`.
pub struct StoreContext<F: StoreFactory = CoreReplStoreFactory> {
    store: MountStore<F>,
    current_path: Path,
    /// Handle to HelpStore state for dynamic updates on mount/unmount
    help_state: Option<HelpStoreHandle>,
    /// Problems met while setting up, for the caller to report.
    warnings: Vec<String>,
}

impl StoreContext<CoreReplStoreFactory> {
    /// Create a new context with the default factory and the
    /// [`DEFAULT_MOUNTS`].
    ///
    /// A default mount that fails to come up is skipped and recorded as a
    /// warning; see [`take_warnings`](Self::take_warnings).
    pub fn new() -> Self {
        Self::with_factory_and_mounts(CoreReplStoreFactory, true)
    }
}

const REGISTERS: [&str; 2] = ["ctx", "registers"];

fn registers_root() -> Path {
    path!("ctx/registers")
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
    match without_at.split_once('/') {
        Some((name, sub_path)) => Some((name.to_string(), Path::parse(sub_path).ok()?)),
        None => Some((without_at.to_string(), path!(""))),
    }
}

/// The store path a register reference (`@`, `@name`, `@name/sub`) names.
pub fn register_store_path(path_str: &str) -> Result<Path, ContextError> {
    let (name, sub_path) = parse_register_path(path_str)
        .ok_or_else(|| ContextError::InvalidPath("Invalid register path".to_string()))?;
    if name.is_empty() {
        return Ok(registers_root());
    }
    let name_path = Path::parse(&name)
        .map_err(|e| ContextError::InvalidPath(format!("Invalid register name: {}", e)))?;
    Ok(registers_root().join(&name_path).join(&sub_path))
}

impl<F: StoreFactory<Config = MountConfig>> StoreContext<F> {
    /// Create a context with a custom factory and optionally the
    /// [`DEFAULT_MOUNTS`].
    ///
    /// If `mount_defaults` is false, the context starts with no mounts. A
    /// default mount that fails is skipped and recorded as a warning.
    pub fn with_factory_and_mounts(factory: F, mount_defaults: bool) -> Self {
        let mut ctx = Self::with_factory(factory);
        if !mount_defaults {
            return ctx;
        }

        for mount in DEFAULT_MOUNTS {
            let result = match mount.kind {
                DefaultMountKind::Config(config) => ctx.store.mount(mount.name, config()),
                DefaultMountKind::Help => {
                    let state = Arc::new(RwLock::new(HelpStoreState::new()));
                    ctx.help_state = Some(Arc::clone(&state));
                    ctx.store
                        .mount_store(mount.name, Box::new(HelpStore::with_shared_state(state)))
                }
            };
            if let Err(e) = result {
                ctx.warnings
                    .push(format!("failed to mount /{}: {}", mount.name, e));
            }
        }
        // The help store is mounted last, so index everything before it.
        for mount in DEFAULT_MOUNTS {
            ctx.index_mount(mount.name);
        }
        ctx
    }
}

impl<F: StoreFactory> StoreContext<F> {
    /// Create a context with a custom factory and no mounts.
    ///
    /// This is useful for testing when you want full control over what stores
    /// are mounted.
    pub fn with_factory(factory: F) -> Self {
        Self {
            store: MountStore::new(factory),
            current_path: path!(""),
            help_state: None,
            warnings: Vec::new(),
        }
    }

    /// Take the warnings recorded since the last call (for example, default
    /// mounts that failed to come up).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Number of mounts, including ones made by writing `ctx/mounts/<name>`.
    pub fn mount_count(&self) -> usize {
        self.store.list_mounts().len()
    }

    /// The shared help state, if the help store is mounted. The state is
    /// plain data updated one topic at a time, so a writer that panicked
    /// mid-update cannot leave anything worth refusing over.
    fn help_guard(&self) -> Option<std::sync::RwLockWriteGuard<'_, HelpStoreState>> {
        self.help_state
            .as_ref()
            .map(|state| state.write().unwrap_or_else(PoisonError::into_inner))
    }

    /// Index one newly mounted store's docs as a help topic.
    ///
    /// Only the new mount's `docs` is read — the index is a cache, and other
    /// mounts are never re-read. A store that serves no docs is no topic.
    fn index_mount(&mut self, name: &str) {
        if self.help_state.is_none() {
            return;
        }
        let Ok(mount_path) = Path::parse(name) else {
            return;
        };
        let from = path!("ctx/help").join(&mount_path);
        let Some((from, to, mode)) = self
            .store
            .list_redirects()
            .into_iter()
            .find(|(redirect, _, _)| *redirect == from)
        else {
            return;
        };
        let Some(manifest) = self
            .store
            .read(&to)
            .ok()
            .flatten()
            .and_then(|record| record.into_value(&NoCodec).ok())
        else {
            return;
        };
        // Topic name: /ctx/help/ctx/sys -> "ctx/sys"
        let topic = mount_path.to_string();
        if let Some(mut guard) = self.help_guard() {
            guard.index_docs(&topic, Some(manifest));
            guard.register_redirect(&topic, &format!("/{from}"), &format!("/{to}"), mode);
        }
    }

    /// Drop an unmounted store's help topic.
    fn unindex_mount(&mut self, name: &str) {
        let Ok(mount_path) = Path::parse(name) else {
            return;
        };
        if let Some(mut guard) = self.help_guard() {
            guard.unindex_docs(&mount_path.to_string());
        }
    }

    /// Mount a store at a path.
    ///
    /// After mounting, the new store's docs (if any) join the help index.
    pub fn mount(&mut self, path: &str, config: F::Config) -> Result<(), ContextError> {
        self.store.mount(path, config)?;
        self.index_mount(path);
        Ok(())
    }

    /// Unmount a store at a path, removing its help topic.
    pub fn unmount(&mut self, path: &str) -> Result<(), ContextError> {
        self.store.unmount(path)?;
        self.unindex_mount(path);
        Ok(())
    }

    /// Read from a register path (`@`, `@name`, `@name/sub/path`).
    ///
    /// Reads from the mounted RegisterStore at `/ctx/registers/`.
    pub fn read_register(&mut self, path_str: &str) -> Result<Option<Value>, ContextError> {
        let register_path = register_store_path(path_str)?;
        self.read(&register_path)
    }

    /// Write to a register path. `@name` replaces the register;
    /// `@name/sub/path` sets a child inside its value.
    ///
    /// Writes to the mounted RegisterStore at `/ctx/registers/`.
    pub fn write_register(&mut self, path_str: &str, value: Value) -> Result<Path, ContextError> {
        let register_path = register_store_path(path_str)?;
        if register_path.len() == REGISTERS.len() {
            return Err(ContextError::InvalidPath(
                "Cannot write to register root. Use @name to specify a register.".to_string(),
            ));
        }
        self.write(&register_path, value)
    }

    /// Store a value directly in a register by name.
    ///
    /// Convenience method that writes to `/ctx/registers/{name}`.
    pub fn set_register(&mut self, name: &str, value: Value) -> Result<(), ContextError> {
        self.write_register(&format!("@{name}"), value).map(|_| ())
    }

    /// Get a value from a register by name.
    ///
    /// Convenience method that reads from `/ctx/registers/{name}`.
    pub fn get_register(&mut self, name: &str) -> Result<Option<Value>, ContextError> {
        self.read_register(&format!("@{name}"))
    }

    /// List all register names.
    ///
    /// Reads from `/ctx/registers/` which returns an array of names.
    pub fn list_registers(&mut self) -> Vec<String> {
        match self.store.read(&registers_root()) {
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
            return Ok(path!(""));
        }

        if let Some(stripped) = path_str.strip_prefix('/') {
            Path::parse(stripped).map_err(|e| ContextError::InvalidPath(format!("{}", e)))
        } else if path_str == ".." {
            let mut components: Vec<String> =
                self.current_path.iter().map(str::to_string).collect();
            components.pop();
            Ok(Path::from_components(components))
        } else if path_str.starts_with("../") {
            let mut components: Vec<String> =
                self.current_path.iter().map(str::to_string).collect();
            let mut remaining = path_str;
            while remaining.starts_with("../") {
                components.pop();
                remaining = &remaining[3..];
            }
            if !remaining.is_empty() {
                let suffix = Path::parse(remaining)
                    .map_err(|e| ContextError::InvalidPath(format!("{}", e)))?;
                components.extend(suffix.iter().map(str::to_string));
            }
            Ok(Path::from_components(components))
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

    /// Write Value to a path.
    ///
    /// Writes to `ctx/mounts/<name>` mount or unmount a store, so they also
    /// add or drop its help topic.
    pub fn write(&mut self, path: &Path, value: Value) -> Result<Path, ContextError> {
        let unmounting = value.is_null();
        let written = self.store.write(path, Record::parsed(value))?;
        if path.len() > 2 && path.has_prefix(&path!("ctx/mounts")) {
            let name = path.slice(2, path.len()).to_string();
            if unmounting {
                self.unindex_mount(&name);
            } else {
                self.index_mount(&name);
            }
        }
        Ok(written)
    }

    /// The child names at a path.
    ///
    /// Paths between mounts (`/`, `/ctx`) are not routed to any store; there
    /// the children are the next components of the mount names.
    pub fn read_children(&mut self, path: &Path) -> Result<Option<Vec<String>>, ContextError> {
        match self.store.read_children(path) {
            Err(CoreError::NoRoute { .. }) => {
                let mut names: Vec<String> = self
                    .store
                    .list_mounts()
                    .into_iter()
                    .filter_map(|(name, _)| Path::parse(&name).ok())
                    .chain(std::iter::once(path!("ctx/mounts")))
                    .filter(|mount| mount.len() > path.len() && mount.has_prefix(path))
                    .map(|mount| mount[path.len()].to_string())
                    .collect();
                names.sort();
                names.dedup();
                if names.is_empty() {
                    // Nothing mounted below either: report the routing error.
                    Ok(self.store.read_children(path)?)
                } else {
                    Ok(Some(names))
                }
            }
            other => Ok(other?),
        }
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

    #[test]
    fn register_store_has_docs() {
        let mut store = RegisterStore::new();
        let result = store.read(&path!("docs")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("title"), Some(&Value::String("Registers".into())));
                assert!(map.contains_key("syntax"));
                assert!(map.contains_key("examples"));
                assert!(map.contains_key("keywords"));
            }
            _ => panic!("Expected map"),
        }
    }

    // StoreContext tests
    #[test]
    fn test_register_write_read() {
        let mut ctx = StoreContext::new();
        ctx.set_register("foo", Value::String("bar".to_string()))
            .unwrap();
        let value = ctx.get_register("foo").unwrap().unwrap();
        assert_eq!(value, Value::String("bar".to_string()));
    }

    #[test]
    fn test_register_list() {
        let mut ctx = StoreContext::new();
        ctx.set_register("a", Value::Integer(1)).unwrap();
        ctx.set_register("b", Value::Integer(2)).unwrap();
        let list = ctx.list_registers();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn set_register_reports_failures() {
        // Without a register store mounted the write has nowhere to go.
        let mut ctx = StoreContext::with_factory(CoreReplStoreFactory);
        assert!(ctx.set_register("a", Value::Integer(1)).is_err());
        assert!(ctx.get_register("a").is_err());
        let mut ctx = StoreContext::new();
        assert!(ctx.set_register("", Value::Integer(1)).is_err());
    }

    #[test]
    fn nested_register_write_sets_a_child() {
        let mut ctx = StoreContext::new();
        ctx.write_register(
            "@foo",
            Value::Map(btree! {"keep".into() => Value::Integer(1)}),
        )
        .unwrap();
        ctx.write_register("@foo/bar", Value::Integer(2)).unwrap();
        assert_eq!(
            ctx.get_register("foo").unwrap(),
            Some(Value::Map(btree! {
                "keep".into() => Value::Integer(1),
                "bar".into() => Value::Integer(2),
            }))
        );
        // A missing register is created as a map.
        ctx.write_register("@fresh/a/b", Value::Bool(true)).unwrap();
        assert_eq!(
            ctx.read_register("@fresh/a/b").unwrap(),
            Some(Value::Bool(true))
        );
        // Setting a child of a scalar is an error, and leaves it intact.
        ctx.write_register("@n", Value::Integer(7)).unwrap();
        let err = ctx.write_register("@n/x", Value::Integer(1)).unwrap_err();
        assert!(matches!(
            err,
            ContextError::Store(CoreError::InvalidArgument { .. })
        ));
        assert_eq!(ctx.get_register("n").unwrap(), Some(Value::Integer(7)));
    }

    #[test]
    fn default_context_has_no_warnings_and_counts_its_mounts() {
        let mut ctx = StoreContext::new();
        assert!(ctx.take_warnings().is_empty());
        assert_eq!(ctx.mount_count(), DEFAULT_MOUNTS.len());
        ctx.mount("extra", MountConfig::Memory).unwrap();
        assert_eq!(ctx.mount_count(), DEFAULT_MOUNTS.len() + 1);
        ctx.write(
            &path!("ctx/mounts/other"),
            MountConfig::Memory.to_value().unwrap(),
        )
        .unwrap();
        assert_eq!(ctx.mount_count(), DEFAULT_MOUNTS.len() + 2);
        ctx.unmount("extra").unwrap();
        assert_eq!(ctx.mount_count(), DEFAULT_MOUNTS.len() + 1);
    }

    #[test]
    fn failed_default_mount_is_a_warning_not_a_print() {
        struct NoSys;
        impl StoreFactory for NoSys {
            type Config = MountConfig;
            fn create(
                &self,
                config: &MountConfig,
            ) -> Result<structfs_core_store::overlay_store::StoreBox, CoreError> {
                if *config == MountConfig::Sys {
                    return Err(CoreError::invalid_argument("no sys here"));
                }
                CoreReplStoreFactory.create(config)
            }
            fn config_from_value(&self, value: Value) -> Result<MountConfig, CoreError> {
                MountConfig::from_value(value)
            }
            fn config_to_value(&self, config: &MountConfig) -> Result<Value, CoreError> {
                config.to_value()
            }
        }
        let mut ctx = StoreContext::with_factory_and_mounts(NoSys, true);
        let warnings = ctx.take_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("/ctx/sys") && warnings[0].contains("no sys here"));
        assert!(ctx.take_warnings().is_empty());
        assert_eq!(ctx.mount_count(), DEFAULT_MOUNTS.len() - 1);
    }

    #[test]
    fn mount_changes_read_only_the_changed_mounts_docs() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// A store whose `docs` reads are counted.
        struct Documented(Arc<AtomicUsize>);
        impl Reader for Documented {
            fn read(&mut self, from: &Path) -> Result<Option<Record>, CoreError> {
                if from.len() == 1 && &from[0] == "docs" {
                    self.0.fetch_add(1, Ordering::SeqCst);
                    return Ok(Some(Record::parsed(Value::Map(btree! {
                        "title".into() => Value::from("Counted"),
                    }))));
                }
                Ok(None)
            }
        }
        impl Writer for Documented {
            fn write(&mut self, to: &Path, _: Record) -> Result<Path, CoreError> {
                Ok(to.clone())
            }
        }
        /// Memory mounts become counted stores sharing one counter.
        struct Counting(Arc<AtomicUsize>);
        impl StoreFactory for Counting {
            type Config = MountConfig;
            fn create(
                &self,
                config: &MountConfig,
            ) -> Result<structfs_core_store::overlay_store::StoreBox, CoreError> {
                match config {
                    MountConfig::Memory => Ok(Box::new(Documented(Arc::clone(&self.0)))),
                    other => CoreReplStoreFactory.create(other),
                }
            }
            fn config_from_value(&self, value: Value) -> Result<MountConfig, CoreError> {
                MountConfig::from_value(value)
            }
            fn config_to_value(&self, config: &MountConfig) -> Result<Value, CoreError> {
                config.to_value()
            }
        }

        let reads = Arc::new(AtomicUsize::new(0));
        let mut ctx = StoreContext::with_factory_and_mounts(Counting(Arc::clone(&reads)), true);
        ctx.mount("a", MountConfig::Memory).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        // Mounting and unmounting others never re-reads `a`.
        ctx.mount("b", MountConfig::Memory).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        ctx.write(&path!("ctx/mounts/c"), MountConfig::Sys.to_value().unwrap())
            .unwrap();
        ctx.unmount("b").unwrap();
        ctx.write(&path!("ctx/mounts/c"), Value::Null).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 2);

        let topics = ctx.read(&path!("ctx/help")).unwrap().unwrap();
        let Value::Array(topics) = topics else {
            panic!("topic list")
        };
        assert!(topics.contains(&Value::from("a")));
        assert!(!topics.contains(&Value::from("b")));
        assert!(!topics.contains(&Value::from("c")));
        assert!(topics.contains(&Value::from("ctx/sys")));
    }

    #[test]
    fn mounting_by_write_updates_help_topics() {
        let mut ctx = StoreContext::new();
        ctx.write(&path!("ctx/mounts/ctx/sys"), Value::Null)
            .unwrap();
        let topics = ctx.read(&path!("ctx/help")).unwrap().unwrap();
        assert!(!matches!(&topics, Value::Array(a) if a.contains(&Value::from("ctx/sys"))));
        ctx.write(
            &path!("ctx/mounts/ctx/sys"),
            MountConfig::Sys.to_value().unwrap(),
        )
        .unwrap();
        let topics = ctx.read(&path!("ctx/help")).unwrap().unwrap();
        assert!(
            matches!(&topics, Value::Array(a) if a.contains(&Value::from("ctx/sys"))),
            "{topics:?}"
        );
    }

    #[test]
    fn read_children_lists_between_mounts() {
        let mut ctx = StoreContext::new();
        assert_eq!(
            ctx.read_children(&path!("")).unwrap(),
            Some(vec!["ctx".to_string()])
        );
        let ctx_children = ctx.read_children(&path!("ctx")).unwrap().unwrap();
        for name in [
            "help",
            "http",
            "http_sync",
            "mounts",
            "registers",
            "repl",
            "sys",
        ] {
            assert!(ctx_children.contains(&name.to_string()), "{ctx_children:?}");
        }
        // Inside a mount, the store answers.
        ctx.mount("data", MountConfig::Memory).unwrap();
        ctx.write(&path!("data/a/x"), Value::Integer(1)).unwrap();
        ctx.write(&path!("data/b"), Value::Integer(2)).unwrap();
        assert_eq!(
            ctx.read_children(&path!("data")).unwrap(),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(ctx.read_children(&path!("data/zzz")).unwrap(), None);
        // Nowhere near a mount is still a routing error.
        assert!(matches!(
            ctx.read_children(&path!("nowhere")),
            Err(ContextError::Store(CoreError::NoRoute { .. }))
        ));
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
        ctx.set_current_path(path!("foo"));
        let path = ctx.resolve_path("bar").unwrap();
        assert_eq!(path.to_string(), "foo/bar");
    }

    #[test]
    fn test_resolve_empty_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(path!("foo"));
        let path = ctx.resolve_path("").unwrap();
        assert_eq!(path.to_string(), "foo");
    }

    #[test]
    fn test_resolve_dot_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(path!("foo"));
        let path = ctx.resolve_path(".").unwrap();
        assert_eq!(path.to_string(), "foo");
    }

    #[test]
    fn test_resolve_root_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(path!("foo/bar"));
        let path = ctx.resolve_path("/").unwrap();
        assert_eq!(path.to_string(), "");
    }

    #[test]
    fn test_resolve_parent_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(path!("foo/bar"));
        let path = ctx.resolve_path("..").unwrap();
        assert_eq!(path.to_string(), "foo");
    }

    #[test]
    fn test_resolve_parent_relative_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(path!("foo/bar/baz"));
        let path = ctx.resolve_path("../qux").unwrap();
        assert_eq!(path.to_string(), "foo/bar/qux");
    }

    #[test]
    fn test_resolve_multiple_parent_path() {
        let mut ctx = StoreContext::new();
        ctx.set_current_path(path!("a/b/c/d"));
        let path = ctx.resolve_path("../../x").unwrap();
        assert_eq!(path.to_string(), "a/b/x");
    }

    #[test]
    fn test_current_path() {
        let mut ctx = StoreContext::new();
        assert_eq!(ctx.current_path().to_string(), "");
        ctx.set_current_path(path!("foo/bar"));
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
        ctx.set_register("test", Value::Integer(42)).unwrap();
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
        ctx.set_register("a", Value::Integer(1)).unwrap();
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
            ctx.get_register("myvar").unwrap(),
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
                // Should include registers (has docs)
                assert!(
                    topics.contains(&Value::String("ctx/registers".into())),
                    "Expected ctx/registers topic, got: {:?}",
                    topics
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

    #[test]
    fn local_and_http_mount_through_the_mount_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.json");
        let mut ctx = StoreContext::new();
        let local = Value::Map(btree! {
            "type".to_string() => Value::from("local"),
            "path".to_string() => Value::from(file.to_string_lossy().into_owned()),
        });
        ctx.write(&path!("ctx/mounts/disk"), local.clone()).unwrap();
        ctx.write(&path!("disk/greeting"), Value::from("hi"))
            .unwrap();
        assert!(file.exists());
        assert_eq!(ctx.read(&path!("ctx/mounts/disk")).unwrap(), Some(local));

        let http = Value::Map(btree! {
            "type".to_string() => Value::from("http"),
            "url".to_string() => Value::from("https://api.example.com"),
        });
        ctx.write(&path!("ctx/mounts/api"), http.clone()).unwrap();
        assert_eq!(ctx.read(&path!("ctx/mounts/api")).unwrap(), Some(http));

        let remote = Value::Map(btree! {
            "type".to_string() => Value::from("structfs"),
            "url".to_string() => Value::from("https://fs.example.com"),
        });
        assert!(matches!(
            ctx.write(&path!("ctx/mounts/remote"), remote),
            Err(ContextError::Store(CoreError::InvalidArgument { .. }))
        ));
    }

    #[test]
    fn list_registers_empty() {
        let mut ctx = StoreContext::new();
        let list = ctx.list_registers();
        assert!(list.is_empty());
    }
}

#[cfg(test)]
mod recording_mount_tests {
    use super::*;
    use structfs_core_store::path;

    /// The whole user journey, minus the terminal: mount a recording
    /// through the mount protocol, browse it, page the timeline, and
    /// find it read-only.
    #[test]
    fn a_recording_mounts_and_pages_through_the_mount_protocol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.jsonl"),
            "{\"seq\":0,\"block\":\"demo/kv\",\"op\":\"read\",\"path\":\"iso/server/requests\",\"outcome\":\"found\",\"entry\":0}\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("demo")).unwrap();
        std::fs::write(
            dir.path().join("demo/kv.transcript.jsonl"),
            "{\"op\":\"read\",\"path\":\"iso/server/requests\",\"answer\":\"absent\"}\n",
        )
        .unwrap();

        let mut ctx = StoreContext::new();
        // `write /ctx/mounts/rec {"type": "recording", "path": DIR}`
        let config = Value::Map(collection_literals::btree! {
            "type".to_string() => Value::from("recording"),
            "path".to_string() => Value::from(dir.path().to_string_lossy().into_owned()),
        });
        ctx.write(&path!("ctx/mounts/rec"), config).unwrap();

        // `read /rec` lists the recording; the timeline pages.
        let root = ctx.read(&path!("rec")).unwrap().unwrap();
        let Value::Map(map) = root else {
            panic!("expected a listing, got {root:?}");
        };
        assert!(map.contains_key("session"), "{map:?}");
        let page = ctx
            .read(&path!("rec/session/entries/from/0"))
            .unwrap()
            .unwrap();
        let Value::Map(envelope) = page else {
            panic!("expected a tail envelope");
        };
        assert!(matches!(
            envelope.get("items"),
            Some(Value::Array(items)) if items.len() == 1
        ));
        // The transcript serves at its key; the recording refuses writes.
        assert!(ctx.read(&path!("rec/demo/kv/entries/0")).unwrap().is_some());
        assert!(ctx
            .write(&path!("rec/session/append"), Value::Null)
            .is_err());

        // A single log mounts standalone with `{"type": "log"}` — and,
        // being a ledger, takes appends.
        let log_config = Value::Map(collection_literals::btree! {
            "type".to_string() => Value::from("log"),
            "path".to_string() => Value::from(
                dir.path().join("notes.jsonl").to_string_lossy().into_owned(),
            ),
        });
        ctx.write(&path!("ctx/mounts/notes"), log_config).unwrap();
        let at = ctx
            .write(&path!("notes/append"), Value::from("first"))
            .unwrap();
        assert_eq!(at, path!("notes/entries/0"));
        assert_eq!(
            ctx.read(&path!("notes/len")).unwrap(),
            Some(Value::Integer(1))
        );
    }
}
