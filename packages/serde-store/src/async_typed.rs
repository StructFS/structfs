//! Async typed reader and writer extension traits.
//!
//! These traits provide typed access to async stores via serde.
//!
//! Enable the `async` feature to use these traits:
//!
//! ```toml
//! [dependencies]
//! structfs-serde-store = { version = "0.1", features = ["async"] }
//! ```

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_core_store::{AsyncReader, AsyncWriter, Codec, Error, Path, Record};

use crate::convert::{from_value, to_value};

/// Async extension trait for typed reads.
///
/// This trait is automatically implemented for all `AsyncReader` implementations.
/// It provides convenience methods for reading data directly into Rust types.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_serde_store::{AsyncTypedReader, JsonCodec};
/// use serde::Deserialize;
///
/// #[derive(Deserialize)]
/// struct Config {
///     debug: bool,
///     port: u16,
/// }
///
/// async fn read_config(store: &mut dyn AsyncReader) -> Result<Config, Error> {
///     let codec = JsonCodec;
///     store.read_as_async::<Config>(&path!("config"), &codec).await?
///         .ok_or_else(|| Error::Other { message: "config not found".into() })
/// }
/// ```
#[async_trait]
pub trait AsyncTypedReader: AsyncReader {
    /// Read a value and deserialize it into a Rust type asynchronously.
    ///
    /// This method:
    /// 1. Reads the Record from the store
    /// 2. Parses it to a Value using the codec (if raw)
    /// 3. Deserializes the Value to the target type
    async fn read_as_async<T: DeserializeOwned + Send>(
        &mut self,
        from: &Path,
        codec: &(dyn Codec + Sync),
    ) -> Result<Option<T>, Error> {
        let Some(record) = self.read_async(from).await? else {
            return Ok(None);
        };

        let value = record.into_value(codec)?;
        let typed = from_value(value)?;
        Ok(Some(typed))
    }

    /// Read a value as a serde_json::Value asynchronously.
    ///
    /// Convenience method when you don't know the exact type.
    async fn read_json_async(
        &mut self,
        from: &Path,
        codec: &(dyn Codec + Sync),
    ) -> Result<Option<serde_json::Value>, Error> {
        self.read_as_async(from, codec).await
    }
}

// Blanket implementation for all AsyncReaders
#[async_trait]
impl<R: AsyncReader + ?Sized + Send> AsyncTypedReader for R {}

/// Async extension trait for typed writes.
///
/// This trait is automatically implemented for all `AsyncWriter` implementations.
/// It provides convenience methods for writing Rust types directly.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_serde_store::AsyncTypedWriter;
/// use serde::Serialize;
///
/// #[derive(Serialize)]
/// struct User {
///     name: String,
///     email: String,
/// }
///
/// async fn create_user(store: &mut dyn AsyncWriter, user: &User) -> Result<Path, Error> {
///     store.write_as_async(&path!("users/new"), user).await
/// }
/// ```
#[async_trait]
pub trait AsyncTypedWriter: AsyncWriter {
    /// Serialize a Rust type and write it to the store asynchronously.
    ///
    /// This method:
    /// 1. Serializes the data to a Value
    /// 2. Wraps it in a Record::Parsed
    /// 3. Writes it to the store
    async fn write_as_async<T: Serialize + Sync>(
        &mut self,
        to: &Path,
        data: &T,
    ) -> Result<Path, Error> {
        let value = to_value(data)?;
        self.write_async(to, Record::parsed(value)).await
    }

    /// Write a serde_json::Value to the store asynchronously.
    ///
    /// Convenience method for dynamic JSON data.
    async fn write_json_async(
        &mut self,
        to: &Path,
        data: serde_json::Value,
    ) -> Result<Path, Error> {
        self.write_as_async(to, &data).await
    }
}

// Blanket implementation for all AsyncWriters
#[async_trait]
impl<W: AsyncWriter + ?Sized + Send> AsyncTypedWriter for W {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JsonCodec;
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use structfs_core_store::{path, Record};

    /// Simple async test store
    struct TestAsyncStore {
        data: HashMap<Path, Record>,
    }

    impl TestAsyncStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    #[async_trait]
    impl AsyncReader for TestAsyncStore {
        async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    #[async_trait]
    impl AsyncWriter for TestAsyncStore {
        async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestUser {
        name: String,
        age: u32,
    }

    #[tokio::test]
    async fn async_typed_roundtrip() {
        let mut store = TestAsyncStore::new();
        let codec = JsonCodec;

        let user = TestUser {
            name: "Alice".to_string(),
            age: 30,
        };

        // Write typed
        store
            .write_as_async(&path!("users/alice"), &user)
            .await
            .unwrap();

        // Read typed
        let recovered: TestUser = store
            .read_as_async(&path!("users/alice"), &codec)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(user, recovered);
    }

    #[tokio::test]
    async fn async_read_nonexistent_returns_none() {
        let mut store = TestAsyncStore::new();
        let codec = JsonCodec;

        let result: Option<TestUser> = store
            .read_as_async(&path!("nonexistent"), &codec)
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn async_write_json_works() {
        let mut store = TestAsyncStore::new();
        let codec = JsonCodec;

        let json = serde_json::json!({
            "key": "value",
            "nested": {"a": 1, "b": 2}
        });

        store
            .write_json_async(&path!("config"), json.clone())
            .await
            .unwrap();

        let recovered: serde_json::Value = store
            .read_json_async(&path!("config"), &codec)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(json, recovered);
    }
}
