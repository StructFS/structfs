//! Environment variable store.

use std::collections::BTreeMap;
use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// Store for environment variable access.
pub struct EnvStore;

impl EnvStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            // Return all environment variables as a map
            let vars: BTreeMap<String, Value> = std::env::vars()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            Ok(Some(Value::Map(vars)))
        } else if path.len() == 1 {
            // Return single variable
            let name = &path[0];
            match std::env::var(name) {
                Ok(value) => Ok(Some(Value::String(value))),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err(Error::store(
                    "env",
                    "read",
                    "Environment variable contains invalid UTF-8",
                )),
            }
        } else {
            Ok(None)
        }
    }
}

impl Default for EnvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for EnvStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for EnvStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::store(
                "env",
                "write",
                "Cannot write to root env path",
            ));
        }

        if to.len() != 1 {
            return Err(Error::store(
                "env",
                "write",
                "Nested environment paths not supported",
            ));
        }

        let name = &to[0];
        let value = data.into_value(&NoCodec)?;

        match value {
            Value::String(s) => {
                std::env::set_var(name, s);
                Ok(to.clone())
            }
            Value::Null => {
                std::env::remove_var(name);
                Ok(to.clone())
            }
            _ => Err(Error::store(
                "env",
                "write",
                "Environment variable must be a string or null",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    #[test]
    fn read_all() {
        let mut store = EnvStore::new();
        let record = store.read(&path!("")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert!(matches!(value, Value::Map(_)));
    }

    #[test]
    fn read_var() {
        std::env::set_var("STRUCTFS_ENV_TEST", "test_value");
        let mut store = EnvStore::new();
        let record = store.read(&path!("STRUCTFS_ENV_TEST")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("test_value".to_string()));
        std::env::remove_var("STRUCTFS_ENV_TEST");
    }

    #[test]
    fn read_nested_path_returns_none() {
        let mut store = EnvStore::new();
        let result = store.read(&path!("HOME/nested/path")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_nonexistent_var() {
        let mut store = EnvStore::new();
        let result = store
            .read(&path!("STRUCTFS_DEFINITELY_NONEXISTENT_VAR"))
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn write_var() {
        let mut store = EnvStore::new();
        let path = path!("STRUCTFS_ENV_WRITE_TEST");
        store
            .write(&path, Record::parsed(Value::String("written".to_string())))
            .unwrap();

        let record = store.read(&path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("written".to_string()));

        // Cleanup
        store.write(&path, Record::parsed(Value::Null)).unwrap();
    }

    #[test]
    fn write_root_error() {
        let mut store = EnvStore::new();
        let result = store.write(&path!(""), Record::parsed(Value::String("x".into())));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot write"));
    }

    #[test]
    fn write_nested_error() {
        let mut store = EnvStore::new();
        let result = store.write(&path!("FOO/BAR"), Record::parsed(Value::String("x".into())));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Nested"));
    }

    #[test]
    fn write_non_string_error() {
        let mut store = EnvStore::new();
        let result = store.write(
            &path!("STRUCTFS_TEST_VAR"),
            Record::parsed(Value::Integer(42)),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be a string"));
    }

    #[test]
    fn default_impl() {
        let _store: EnvStore = Default::default();
    }
}
