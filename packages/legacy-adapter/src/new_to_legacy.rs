//! Adapter to wrap new core-store stores for use with legacy traits.

use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_core_store::{NoCodec, Reader as CoreReader, Record, Value, Writer as CoreWriter};
use structfs_serde_store::{from_value, to_value};
use structfs_store::{Error as LegacyError, Path as LegacyPath};

use crate::path_convert::{core_path_to_legacy, legacy_path_to_core};
use crate::Error;

/// Wraps a new core-store to implement legacy store traits.
///
/// This adapter allows new stores (implementing `structfs_core_store::Reader` and
/// `structfs_core_store::Writer`) to be used with the legacy `structfs_store` traits.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_legacy_adapter::NewToLegacy;
/// use structfs_store::{Reader, path};
///
/// let new_store = MyNewStore::new();
/// let mut legacy_store = NewToLegacy::new(new_store);
///
/// // Now use with legacy store API
/// let value: MyType = legacy_store.read_owned(&path!("foo/bar"))?.unwrap();
/// ```
pub struct NewToLegacy<S> {
    inner: S,
}

impl<S> NewToLegacy<S> {
    /// Create a new adapter wrapping the given new store.
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Get a reference to the inner store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Get a mutable reference to the inner store.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Unwrap and return the inner store.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: CoreReader> structfs_store::Reader for NewToLegacy<S> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &LegacyPath,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, LegacyError>
    where
        'this: 'de,
    {
        // Convert path
        let core_path = legacy_path_to_core(from).map_err(LegacyError::from)?;

        // Read Record from new store
        let maybe_record = self
            .inner
            .read(&core_path)
            .map_err(Error::NewStore)
            .map_err(LegacyError::from)?;

        match maybe_record {
            Some(record) => {
                // Convert Record to Value
                let value = record.into_value(&NoCodec).map_err(|e| {
                    LegacyError::RecordDeserialization {
                        message: format!("Cannot convert record to value: {}", e),
                    }
                })?;

                // Convert Value to serde_json::Value
                let json = value_to_json(value);

                // Create an erased deserializer from the JSON value
                let de: Box<dyn erased_serde::Deserializer<'de>> =
                    Box::new(<dyn erased_serde::Deserializer>::erase(json));

                Ok(Some(de))
            }
            None => Ok(None),
        }
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &LegacyPath,
    ) -> Result<Option<RecordType>, LegacyError> {
        // Convert path
        let core_path = legacy_path_to_core(from).map_err(LegacyError::from)?;

        // Read Record from new store
        let maybe_record = self
            .inner
            .read(&core_path)
            .map_err(Error::NewStore)
            .map_err(LegacyError::from)?;

        match maybe_record {
            Some(record) => {
                // Convert Record to Value
                let value = record.into_value(&NoCodec).map_err(|e| {
                    LegacyError::RecordDeserialization {
                        message: format!("Cannot convert record to value: {}", e),
                    }
                })?;

                // Use serde-store's from_value to deserialize
                let typed: RecordType =
                    from_value(value).map_err(|e| LegacyError::RecordDeserialization {
                        message: e.to_string(),
                    })?;

                Ok(Some(typed))
            }
            None => Ok(None),
        }
    }
}

impl<S: CoreWriter> structfs_store::Writer for NewToLegacy<S> {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &LegacyPath,
        data: RecordType,
    ) -> Result<LegacyPath, LegacyError> {
        // Convert path
        let core_path = legacy_path_to_core(destination).map_err(LegacyError::from)?;

        // Convert data to Value via serde-store
        let value = to_value(&data).map_err(|e| LegacyError::RecordSerialization {
            message: e.to_string(),
        })?;

        // Create Record from Value
        let record = Record::parsed(value);

        // Write using new API
        let result_path = self
            .inner
            .write(&core_path, record)
            .map_err(Error::NewStore)
            .map_err(LegacyError::from)?;

        // Convert result path back
        core_path_to_legacy(&result_path).map_err(LegacyError::from)
    }
}

/// Convert core_store::Value to serde_json::Value.
fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        Value::Integer(i) => serde_json::Value::Number(i.into()),
        Value::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s),
        Value::Bytes(b) => {
            // JSON doesn't have bytes, so base64 encode
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&b);
            serde_json::Value::String(encoded)
        }
        Value::Array(arr) => serde_json::Value::Array(arr.into_iter().map(value_to_json).collect()),
        Value::Map(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, value_to_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use structfs_core_store::Path as CorePath;
    use structfs_store::{Reader as LegacyReader, Writer as LegacyWriter};

    // Simple in-memory store for testing
    struct TestStore {
        data: HashMap<CorePath, Record>,
    }

    impl TestStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl CoreReader for TestStore {
        fn read(&mut self, from: &CorePath) -> Result<Option<Record>, structfs_core_store::Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl CoreWriter for TestStore {
        fn write(
            &mut self,
            to: &CorePath,
            data: Record,
        ) -> Result<CorePath, structfs_core_store::Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    // Make it Send + Sync for the adapter
    unsafe impl Send for TestStore {}
    unsafe impl Sync for TestStore {}

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct User {
        name: String,
        age: u32,
    }

    #[test]
    fn write_and_read_typed() {
        let mut store = NewToLegacy::new(TestStore::new());

        let path = structfs_store::Path::parse("users/alice").unwrap();
        let user = User {
            name: "Alice".to_string(),
            age: 30,
        };

        // Write using legacy API
        store.write(&path, &user).unwrap();

        // Read back using legacy API
        let read_user: User = store.read_owned(&path).unwrap().unwrap();
        assert_eq!(read_user, user);
    }

    #[test]
    fn read_none_for_missing() {
        let mut store = NewToLegacy::new(TestStore::new());
        let path = structfs_store::Path::parse("nonexistent").unwrap();

        let result: Option<String> = store.read_owned(&path).unwrap();
        assert!(result.is_none());
    }
}
