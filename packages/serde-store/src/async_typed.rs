//! Async typed reader and writer extension traits.
//!
//! These traits provide typed access to async stores via serde. They mirror
//! the synchronous [`TypedReader`](crate::TypedReader) /
//! [`TypedWriter`](crate::TypedWriter) surface one method at a time; see the
//! [crate docs](crate#typed-access) for the matrix and for the codec
//! convention.
//!
//! Enable the `async` feature to use these traits:
//!
//! ```toml
//! [dependencies]
//! structfs-serde-store = { version = "0.4", features = ["async"] }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_core_store::{AsyncReader, AsyncWriter, Codec, Error, Path, Record};

use crate::convert::{from_value, to_value};

/// Async extension trait for typed reads.
///
/// This trait is automatically implemented for all `AsyncReader` implementations.
///
/// There is no `read_children_typed_async`: [`AsyncReader`] has no child
/// enumeration to build it on. When core-store grows one, this trait grows
/// the typed wrapper.
///
/// # Example
///
/// ```rust
/// # #[cfg(feature = "async")] {
/// use std::sync::Arc;
/// use serde::{Deserialize, Serialize};
/// use structfs_core_store::{path, Codec, Error, MemoryStore, SyncToAsync};
/// use structfs_serde_store::{AsyncTypedReader, AsyncTypedWriter, JsonCodec};
///
/// #[derive(Debug, PartialEq, Serialize, Deserialize)]
/// struct Config { debug: bool, port: u16 }
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let mut store = SyncToAsync::new(MemoryStore::new());
/// store.write_typed_async(&path!("config"), &Config { debug: true, port: 8080 }).await?;
///
/// let config: Config = store.read_typed_async(&path!("config")).await?.unwrap();
/// assert_eq!(config.port, 8080);
///
/// let codec: Arc<dyn Codec> = Arc::new(JsonCodec);
/// let config: Option<Config> = store.read_as_async(&path!("config"), codec).await?;
/// assert!(config.is_some());
/// # Ok::<(), Error>(())
/// # }).unwrap();
/// # }
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
        codec: Arc<dyn Codec>,
    ) -> Result<Option<T>, Error> {
        let Some(record) = self.read_async(from).await? else {
            return Ok(None);
        };

        let value = record.into_value(codec.as_ref())?;
        let typed = from_value(value)?;
        Ok(Some(typed))
    }

    /// Parsed-only typed read, identical to synchronous and detached helpers.
    async fn read_typed_async<T: DeserializeOwned + Send>(
        &mut self,
        from: &Path,
    ) -> Result<Option<T>, Error> {
        let Some(record) = self.read_async(from).await? else {
            return Ok(None);
        };
        Ok(Some(from_value(
            record.into_value(&structfs_core_store::NoCodec)?,
        )?))
    }
}

// Blanket implementation for all AsyncReaders
#[async_trait]
impl<R: AsyncReader + ?Sized + Send> AsyncTypedReader for R {}

/// Async extension trait for typed writes.
///
/// This trait is automatically implemented for all `AsyncWriter` implementations.
/// As in the synchronous flavour, there is no codec-taking counterpart: a
/// typed write always produces a parsed record.
#[async_trait]
pub trait AsyncTypedWriter: AsyncWriter {
    /// Serialize a Rust type and write it to the store asynchronously.
    ///
    /// This method:
    /// 1. Serializes the data to a Value
    /// 2. Wraps it in a Record::Parsed
    /// 3. Writes it to the store
    async fn write_typed_async<T: Serialize + Sync + ?Sized>(
        &mut self,
        to: &Path,
        data: &T,
    ) -> Result<Path, Error> {
        let value = to_value(data)?;
        self.write_async(to, Record::parsed(value)).await
    }
}

// Blanket implementation for all AsyncWriters
#[async_trait]
impl<W: AsyncWriter + ?Sized + Send> AsyncTypedWriter for W {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::typed::tests::{alice, raw_alice, RecordStore, TestUser};
    use crate::JsonCodec;
    use structfs_core_store::{path, MemoryStore, SyncToAsync, Writer};

    fn store() -> SyncToAsync<MemoryStore> {
        SyncToAsync::new(MemoryStore::new())
    }

    #[tokio::test]
    async fn async_typed_roundtrip_is_codec_free() {
        let mut store = store();
        store
            .write_typed_async(&path!("users/alice"), &alice())
            .await
            .unwrap();

        let recovered: TestUser = store
            .read_typed_async(&path!("users/alice"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered, alice());

        let missing: Option<TestUser> = store.read_typed_async(&path!("nope")).await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn async_read_as_supplies_the_codec_raw_records_need() {
        let mut inner = RecordStore::default();
        inner.write(&path!("raw"), raw_alice()).unwrap();
        let mut store = SyncToAsync::new(inner);

        assert!(store
            .read_typed_async::<TestUser>(&path!("raw"))
            .await
            .is_err());

        let codec: Arc<dyn Codec> = Arc::new(JsonCodec);
        let user: TestUser = store
            .read_as_async(&path!("raw"), codec.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user, alice());

        let missing: Option<TestUser> = store
            .read_as_async(&path!("nonexistent"), codec)
            .await
            .unwrap();
        assert!(missing.is_none());
    }
}
