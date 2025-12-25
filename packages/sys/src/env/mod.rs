//! Environment variable store.
//!
//! Provides access to environment variables through StructFS paths.
//!
//! ## Paths
//!
//! - `env/` - Read returns all environment variables as an object
//! - `env/{NAME}` - Read/write individual variable

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;
use structfs_store::{Error, Path, Reader, Writer};

/// Store for environment variable access.
pub struct EnvStore;

impl EnvStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.is_empty() {
            // Return all environment variables as an object
            let vars: serde_json::Map<String, JsonValue> = std::env::vars()
                .map(|(k, v)| (k, JsonValue::String(v)))
                .collect();
            Ok(Some(JsonValue::Object(vars)))
        } else if path.components.len() == 1 {
            // Return single variable
            let name = &path.components[0];
            match std::env::var(name) {
                Ok(value) => Ok(Some(JsonValue::String(value))),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err(Error::ImplementationFailure {
                    message: "Environment variable contains invalid UTF-8".to_string(),
                }),
            }
        } else {
            // Nested paths not supported
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
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
        Ok(self.read_value(from)?.map(|v| {
            let de: Box<dyn erased_serde::Deserializer> =
                Box::new(<dyn erased_serde::Deserializer>::erase(v));
            de
        }))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error> {
        if let Some(value) = self.read_value(from)? {
            let data: RecordType =
                serde_json::from_value(value).map_err(|err| Error::RecordDeserialization {
                    message: format!("Failed to deserialize env value: {}", err),
                })?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

impl Writer for EnvStore {
    fn write<RecordType: Serialize>(
        &mut self,
        path: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        if path.is_empty() {
            return Err(Error::ImplementationFailure {
                message: "Cannot write to root env path".to_string(),
            });
        }

        if path.components.len() != 1 {
            return Err(Error::ImplementationFailure {
                message: "Nested environment paths not supported".to_string(),
            });
        }

        let name = &path.components[0];
        let value = serde_json::to_value(data).map_err(|err| Error::RecordSerialization {
            message: format!("Failed to serialize env value: {}", err),
        })?;

        match value {
            JsonValue::String(s) => {
                std::env::set_var(name, s);
                Ok(path.clone())
            }
            JsonValue::Null => {
                std::env::remove_var(name);
                Ok(path.clone())
            }
            _ => Err(Error::ImplementationFailure {
                message: "Environment variable must be a string or null".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_all_vars() {
        let mut store = EnvStore::new();
        let result: Option<serde_json::Map<String, JsonValue>> =
            store.read_owned(&Path::parse("").unwrap()).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_read_path() {
        std::env::set_var("STRUCTFS_TEST_VAR", "test_value");
        let mut store = EnvStore::new();
        let result: Option<String> = store
            .read_owned(&Path::parse("STRUCTFS_TEST_VAR").unwrap())
            .unwrap();
        assert_eq!(result, Some("test_value".to_string()));
        std::env::remove_var("STRUCTFS_TEST_VAR");
    }

    #[test]
    fn test_read_missing() {
        let mut store = EnvStore::new();
        let result: Option<String> = store
            .read_owned(&Path::parse("STRUCTFS_DEFINITELY_NOT_SET").unwrap())
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_write_and_read() {
        let mut store = EnvStore::new();
        let path = Path::parse("STRUCTFS_WRITE_TEST").unwrap();

        store.write(&path, "written_value").unwrap();

        let result: Option<String> = store.read_owned(&path).unwrap();
        assert_eq!(result, Some("written_value".to_string()));

        // Clean up - write null to remove
        store.write(&path, JsonValue::Null).unwrap();
    }
}
