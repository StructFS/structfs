//! Environment variable store.

use std::collections::BTreeMap;

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// Store for environment variables.
///
/// Reads see the process environment with this store's overlay applied.
/// Writes never call `std::env::set_var`/`remove_var` — mutating the
/// process environment while other threads may read it is undefined
/// behaviour on POSIX platforms. Instead, writing a string sets the
/// variable in an in-process overlay and writing `null` hides it; the
/// overlay is private to this store (other stores, `proc/self/env`, and
/// child processes see the real environment). Use
/// [`overrides`](Self::overrides) to apply the overlay when spawning a
/// child process.
#[derive(Debug, Default)]
pub struct EnvStore {
    /// `Some(value)` sets a variable; `None` hides it.
    overlay: BTreeMap<String, Option<String>>,
}

impl EnvStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The variables written through this store: `Some` values were set,
    /// `None` entries were unset.
    pub fn overrides(&self) -> &BTreeMap<String, Option<String>> {
        &self.overlay
    }

    fn var(&self, name: &str) -> Result<Option<String>, Error> {
        if let Some(value) = self.overlay.get(name) {
            return Ok(value.clone());
        }
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(Error::store(
                "env",
                "read",
                format!("environment variable {name} is not valid UTF-8"),
            )),
        }
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        match path.len() {
            0 => {
                let mut vars: BTreeMap<String, Value> = std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect();
                for (name, value) in &self.overlay {
                    match value {
                        Some(value) => vars.insert(name.clone(), Value::String(value.clone())),
                        None => vars.remove(name),
                    };
                }
                Ok(Some(Value::Map(vars)))
            }
            1 => Ok(self.var(&path[0])?.map(Value::String)),
            _ => Ok(None),
        }
    }
}

impl Reader for EnvStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for EnvStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() != 1 {
            return Err(Error::invalid_argument(
                "write a string or null to env/NAME",
            ));
        }
        let value = match data.into_value(&NoCodec)? {
            Value::String(s) => Some(s),
            Value::Null => None,
            _ => {
                return Err(Error::invalid_argument(
                    "environment variable must be a string or null",
                ))
            }
        };
        self.overlay.insert(to[0].to_string(), value);
        Ok(to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    fn read(store: &mut EnvStore, at: &Path) -> Option<Value> {
        store
            .read(at)
            .unwrap()
            .map(|r| r.into_value(&NoCodec).unwrap())
    }

    #[test]
    fn reads_process_environment() {
        let mut store = EnvStore::new();
        assert_eq!(
            read(&mut store, &path!("CARGO_PKG_NAME")),
            Some(Value::String("structfs-sys".into()))
        );
        assert!(
            matches!(read(&mut store, &path!("")), Some(Value::Map(m)) if m.contains_key("CARGO_PKG_NAME"))
        );
        assert_eq!(read(&mut store, &path!("STRUCTFS_DEFINITELY_UNSET")), None);
        assert_eq!(read(&mut store, &path!("CARGO_PKG_NAME/nested")), None);
    }

    #[test]
    fn writes_go_to_the_overlay() {
        let mut store = EnvStore::new();
        store
            .write(
                &path!("STRUCTFS_ENV_OVERLAY"),
                Record::parsed(Value::String("x".into())),
            )
            .unwrap();
        assert_eq!(
            read(&mut store, &path!("STRUCTFS_ENV_OVERLAY")),
            Some(Value::String("x".into()))
        );
        assert!(std::env::var("STRUCTFS_ENV_OVERLAY").is_err());
        assert!(
            matches!(read(&mut store, &path!("")), Some(Value::Map(m)) if m.contains_key("STRUCTFS_ENV_OVERLAY"))
        );

        // Unsetting hides a real variable without touching the process.
        store
            .write(&path!("CARGO_PKG_NAME"), Record::parsed(Value::Null))
            .unwrap();
        assert_eq!(read(&mut store, &path!("CARGO_PKG_NAME")), None);
        assert!(
            matches!(read(&mut store, &path!("")), Some(Value::Map(m)) if !m.contains_key("CARGO_PKG_NAME"))
        );
        assert!(std::env::var("CARGO_PKG_NAME").is_ok());
        assert_eq!(
            store.overrides().get("CARGO_PKG_NAME"),
            Some(&None::<String>)
        );
    }

    #[test]
    fn invalid_writes_are_invalid_argument() {
        let mut store = EnvStore::new();
        for (at, value) in [
            (path!(""), Value::String("x".into())),
            (path!("FOO/BAR"), Value::String("x".into())),
            (path!("FOO"), Value::Integer(42)),
        ] {
            assert!(matches!(
                store.write(&at, Record::parsed(value)),
                Err(Error::InvalidArgument { .. })
            ));
        }
    }
}
