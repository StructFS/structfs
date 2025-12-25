//! Typed reader and writer extension traits.

use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_core_store::{Codec, Error, Path, Reader, Record, Writer};

use crate::convert::{from_value, to_value};

/// Extension trait for typed reads.
///
/// This trait is automatically implemented for all `Reader` implementations.
/// It provides convenience methods for reading data directly into Rust types.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_serde_store::{TypedReader, JsonCodec};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Config {
///     debug: bool,
///     port: u16,
/// }
///
/// fn read_config(store: &mut dyn Reader) -> Result<Config, Error> {
///     let codec = JsonCodec;
///     store.read_as(&path!("config"), &codec)?
///         .ok_or_else(|| Error::Other { message: "config not found".into() })
/// }
/// ```
pub trait TypedReader: Reader {
    /// Read a value and deserialize it into a Rust type.
    ///
    /// This method:
    /// 1. Reads the Record from the store
    /// 2. Parses it to a Value using the codec (if raw)
    /// 3. Deserializes the Value to the target type
    fn read_as<T: DeserializeOwned>(
        &mut self,
        from: &Path,
        codec: &dyn Codec,
    ) -> Result<Option<T>, Error> {
        let Some(record) = self.read(from)? else {
            return Ok(None);
        };

        let value = record.into_value(codec)?;
        let typed = from_value(value)?;
        Ok(Some(typed))
    }

    /// Read a value as a serde_json::Value.
    ///
    /// Convenience method when you don't know the exact type.
    fn read_json(
        &mut self,
        from: &Path,
        codec: &dyn Codec,
    ) -> Result<Option<serde_json::Value>, Error> {
        self.read_as(from, codec)
    }
}

// Blanket implementation for all Readers
impl<R: Reader + ?Sized> TypedReader for R {}

/// Extension trait for typed writes.
///
/// This trait is automatically implemented for all `Writer` implementations.
/// It provides convenience methods for writing Rust types directly.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_serde_store::TypedWriter;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct User {
///     name: String,
///     email: String,
/// }
///
/// fn create_user(store: &mut dyn Writer, user: &User) -> Result<Path, Error> {
///     store.write_as(&path!("users/new"), user)
/// }
/// ```
pub trait TypedWriter: Writer {
    /// Serialize a Rust type and write it to the store.
    ///
    /// This method:
    /// 1. Serializes the data to a Value
    /// 2. Wraps it in a Record::Parsed
    /// 3. Writes it to the store
    fn write_as<T: Serialize>(&mut self, to: &Path, data: &T) -> Result<Path, Error> {
        let value = to_value(data)?;
        self.write(to, Record::parsed(value))
    }

    /// Write a serde_json::Value to the store.
    ///
    /// Convenience method for dynamic JSON data.
    fn write_json(&mut self, to: &Path, data: serde_json::Value) -> Result<Path, Error> {
        self.write_as(to, &data)
    }
}

// Blanket implementation for all Writers
impl<W: Writer + ?Sized> TypedWriter for W {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    /// Simple test store
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

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestUser {
        name: String,
        age: u32,
    }

    #[test]
    fn typed_roundtrip() {
        use structfs_core_store::path;

        let mut store = TestStore::new();
        let codec = crate::JsonCodec;

        let user = TestUser {
            name: "Alice".to_string(),
            age: 30,
        };

        // Write typed
        store.write_as(&path!("users/alice"), &user).unwrap();

        // Read typed
        let recovered: TestUser = store
            .read_as(&path!("users/alice"), &codec)
            .unwrap()
            .unwrap();

        assert_eq!(user, recovered);
    }

    #[test]
    fn read_nonexistent_returns_none() {
        use structfs_core_store::path;

        let mut store = TestStore::new();
        let codec = crate::JsonCodec;

        let result: Option<TestUser> = store.read_as(&path!("nonexistent"), &codec).unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn write_json_works() {
        use structfs_core_store::path;

        let mut store = TestStore::new();
        let codec = crate::JsonCodec;

        let json = serde_json::json!({
            "key": "value",
            "nested": {"a": 1, "b": 2}
        });

        store.write_json(&path!("config"), json.clone()).unwrap();

        let recovered: serde_json::Value =
            store.read_json(&path!("config"), &codec).unwrap().unwrap();

        assert_eq!(json, recovered);
    }
}
