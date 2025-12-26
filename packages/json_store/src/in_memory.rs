//! In-memory store using the new core-store architecture.
//!
//! In-memory JSON store using Value type.

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

use crate::value_utils;

/// An in-memory store using core_store::Value as the storage format.
///
/// In-memory store that uses the core-store Value type.
///
/// # Example
///
/// ```rust
/// use structfs_json_store::InMemoryStore;
/// use structfs_core_store::{Reader, Writer, Record, Value, path};
///
/// let mut store = InMemoryStore::new();
///
/// // Write a value at the root
/// store.write(&path!("name"), Record::parsed(Value::String("Alice".to_string()))).unwrap();
///
/// // Read it back
/// let record = store.read(&path!("name")).unwrap().unwrap();
/// let value = record.into_value(&structfs_core_store::NoCodec).unwrap();
/// assert_eq!(value, Value::String("Alice".to_string()));
/// ```
pub struct InMemoryStore {
    root: Value,
}

impl InMemoryStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self { root: Value::Null }
    }

    /// Create a store with initial data.
    pub fn with_data(root: Value) -> Self {
        Self { root }
    }

    /// Get a reference to the root value.
    pub fn root(&self) -> &Value {
        &self.root
    }

    /// Get a mutable reference to the root value.
    pub fn root_mut(&mut self) -> &mut Value {
        &mut self.root
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for InMemoryStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match value_utils::get_path(&self.root, from)? {
            Some(value) => {
                let cloned: Value = value.clone();
                Ok(Some(Record::parsed(cloned)))
            }
            None => Ok(None),
        }
    }
}

impl Writer for InMemoryStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;
        value_utils::set_path(&mut self.root, to, value)?;
        Ok(to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use structfs_core_store::path;

    #[test]
    fn basic_write_read() {
        let mut store = InMemoryStore::new();

        store
            .write(
                &path!("foo"),
                Record::parsed(Value::String("bar".to_string())),
            )
            .unwrap();

        let record = store.read(&path!("foo")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("bar".to_string()));
    }

    #[test]
    fn nested_write_read() {
        let mut store = InMemoryStore::new();

        // First create parent
        store
            .write(&path!("users"), Record::parsed(Value::Map(BTreeMap::new())))
            .unwrap();

        // Then write child
        store
            .write(
                &path!("users/alice"),
                Record::parsed(Value::String("Alice".to_string())),
            )
            .unwrap();

        let record = store.read(&path!("users/alice")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("Alice".to_string()));
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let mut store = InMemoryStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn overwrite_works() {
        let mut store = InMemoryStore::new();

        store
            .write(
                &path!("value"),
                Record::parsed(Value::String("first".to_string())),
            )
            .unwrap();

        store
            .write(
                &path!("value"),
                Record::parsed(Value::String("second".to_string())),
            )
            .unwrap();

        let record = store.read(&path!("value")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("second".to_string()));
    }

    #[test]
    fn complex_structure() {
        let mut store = InMemoryStore::new();

        // Build a complex structure
        let mut user = BTreeMap::new();
        user.insert("name".to_string(), Value::String("Alice".to_string()));
        user.insert("age".to_string(), Value::Integer(30));
        user.insert(
            "hobbies".to_string(),
            Value::Array(vec![
                Value::String("reading".to_string()),
                Value::String("coding".to_string()),
            ]),
        );

        store
            .write(&path!(""), Record::parsed(Value::Map(user)))
            .unwrap();

        // Read various paths
        let name = store.read(&path!("name")).unwrap().unwrap();
        assert_eq!(
            name.into_value(&NoCodec).unwrap(),
            Value::String("Alice".to_string())
        );

        let age = store.read(&path!("age")).unwrap().unwrap();
        assert_eq!(age.into_value(&NoCodec).unwrap(), Value::Integer(30));

        let hobby = store.read(&path!("hobbies/0")).unwrap().unwrap();
        assert_eq!(
            hobby.into_value(&NoCodec).unwrap(),
            Value::String("reading".to_string())
        );
    }

    #[test]
    fn with_data_constructor() {
        let mut data = BTreeMap::new();
        data.insert("key".to_string(), Value::String("value".to_string()));

        let mut store = InMemoryStore::with_data(Value::Map(data));

        let record = store.read(&path!("key")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("value".to_string()));
    }
}
