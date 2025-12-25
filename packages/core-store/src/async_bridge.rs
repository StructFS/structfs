//! Async bridges between LL and Core layers.
//!
//! These adapters allow using async LL stores from the async Core layer and vice versa.

use async_trait::async_trait;
use bytes::Bytes;
use structfs_ll_store::{AsyncLLReader, AsyncLLWriter, LLError, LLPath};

use crate::{
    async_traits::{AsyncReader, AsyncWriter},
    bridge::{path_from_bytes, path_from_ll},
    Codec, Error, Format, Path, Record,
};

/// Async adapter: wraps an async LL store to provide the async Core interface.
///
/// This bridge:
/// - Converts `Path` to `&[&[u8]]` for LL operations
/// - Wraps returned bytes as `Record::Raw` with a format hint
/// - Serializes `Record` to bytes for writes
///
/// # Example
///
/// ```rust,ignore
/// use structfs_core_store::{AsyncLLToCore, AsyncReader, Format, path};
///
/// let ll_store = MyAsyncLLStore::new();
/// let mut core_store = AsyncLLToCore::new(ll_store, JsonCodec, Format::JSON);
///
/// let record = core_store.read_async(&path!("users/123")).await?;
/// ```
pub struct AsyncLLToCore<T, C> {
    inner: T,
    codec: C,
    read_format: Format,
    write_format: Format,
}

impl<T, C> AsyncLLToCore<T, C> {
    /// Create a new async bridge with the same format for reads and writes.
    pub fn new(inner: T, codec: C, format: Format) -> Self {
        Self {
            inner,
            codec,
            read_format: format.clone(),
            write_format: format,
        }
    }

    /// Create a new async bridge with different formats for reads and writes.
    pub fn with_formats(inner: T, codec: C, read_format: Format, write_format: Format) -> Self {
        Self {
            inner,
            codec,
            read_format,
            write_format,
        }
    }

    /// Get a reference to the inner LL store.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner LL store.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Unwrap, returning the inner LL store.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

#[async_trait]
impl<T: AsyncLLReader, C: Send + Sync> AsyncReader for AsyncLLToCore<T, C> {
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Convert Path to Vec<Vec<u8>> (need owned data for async)
        let components: Vec<Vec<u8>> = from
            .components
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();

        // Create slice of slices for LL call
        let refs: Vec<&[u8]> = components.iter().map(|v| v.as_slice()).collect();

        // Read via async LL
        let bytes = match self.inner.ll_read_async(&refs).await {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(None),
            Err(e) => return Err(Error::Ll(e)),
        };

        // Wrap as Raw record with our format hint
        Ok(Some(Record::raw(bytes, self.read_format.clone())))
    }
}

#[async_trait]
impl<T: AsyncLLWriter, C: Codec + Send + Sync> AsyncWriter for AsyncLLToCore<T, C> {
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // Get bytes from Record (serialize if Parsed)
        let bytes = data.into_bytes(&self.codec, &self.write_format)?;

        // Convert Path to Vec<Vec<u8>>
        let components: Vec<Vec<u8>> = to
            .components
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect();

        let refs: Vec<&[u8]> = components.iter().map(|v| v.as_slice()).collect();

        // Write via async LL
        let result_path = self
            .inner
            .ll_write_async(&refs, bytes)
            .await
            .map_err(Error::Ll)?;

        // Convert result back to Path
        path_from_ll(&result_path)
    }
}

/// Async adapter: wraps an async Core store to provide the async LL interface.
///
/// This bridge:
/// - Converts `&[&[u8]]` paths to validated `Path`
/// - Parses/serializes data as needed
/// - Returns bytes in the configured format
///
/// # Example
///
/// ```rust,ignore
/// use structfs_core_store::{AsyncCoreToLL, AsyncLLReader, Format};
///
/// let core_store = MyAsyncCoreStore::new();
/// let mut ll_store = AsyncCoreToLL::new(core_store, JsonCodec, Format::JSON);
///
/// let bytes = ll_store.ll_read_async(&[b"users", b"123"]).await?;
/// ```
pub struct AsyncCoreToLL<T, C> {
    inner: T,
    codec: C,
    format: Format,
}

impl<T, C> AsyncCoreToLL<T, C> {
    /// Create a new async bridge.
    pub fn new(inner: T, codec: C, format: Format) -> Self {
        Self {
            inner,
            codec,
            format,
        }
    }

    /// Get a reference to the inner Core store.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner Core store.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Unwrap, returning the inner Core store.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

#[async_trait]
impl<T: AsyncReader, C: Codec + Send + Sync> AsyncLLReader for AsyncCoreToLL<T, C> {
    async fn ll_read_async(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Convert &[&[u8]] to Path
        let path = path_from_bytes(path).map_err(|e| LLError::Protocol {
            code: 1,
            detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
        })?;

        // Read via async Core
        let record = match self.inner.read_async(&path).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => {
                return Err(LLError::Protocol {
                    code: 2,
                    detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
                })
            }
        };

        // Convert to bytes
        let bytes =
            record
                .into_bytes(&self.codec, &self.format)
                .map_err(|e| LLError::Protocol {
                    code: 3,
                    detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
                })?;

        Ok(Some(bytes))
    }
}

#[async_trait]
impl<T: AsyncWriter, C: Send + Sync> AsyncLLWriter for AsyncCoreToLL<T, C> {
    async fn ll_write_async(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        // Convert path
        let path = path_from_bytes(path).map_err(|e| LLError::Protocol {
            code: 1,
            detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
        })?;

        // Wrap data as Raw record
        let record = Record::raw(data, self.format.clone());

        // Write via async Core
        let result_path =
            self.inner
                .write_async(&path, record)
                .await
                .map_err(|e| LLError::Protocol {
                    code: 2,
                    detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
                })?;

        // Convert result to LL path
        Ok(result_path.to_ll_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, NoCodec};
    use std::collections::HashMap;

    /// Simple async in-memory LL store for testing.
    struct TestAsyncLLStore {
        data: HashMap<Vec<Vec<u8>>, Bytes>,
    }

    impl TestAsyncLLStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    #[async_trait]
    impl AsyncLLReader for TestAsyncLLStore {
        async fn ll_read_async(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
            let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
            Ok(self.data.get(&key).cloned())
        }
    }

    #[async_trait]
    impl AsyncLLWriter for TestAsyncLLStore {
        async fn ll_write_async(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
            let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
            self.data.insert(key, data);
            Ok(path.iter().map(|c| Bytes::copy_from_slice(c)).collect())
        }
    }

    /// Simple async in-memory Core store for testing.
    struct TestAsyncCoreStore {
        data: HashMap<Path, Record>,
    }

    impl TestAsyncCoreStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    #[async_trait]
    impl AsyncReader for TestAsyncCoreStore {
        async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    #[async_trait]
    impl AsyncWriter for TestAsyncCoreStore {
        async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[tokio::test]
    async fn async_ll_to_core_read() {
        let mut ll = TestAsyncLLStore::new();
        ll.data.insert(
            vec![b"users".to_vec(), b"123".to_vec()],
            Bytes::from_static(b"hello"),
        );

        let mut bridge = AsyncLLToCore::new(ll, NoCodec, Format::OCTET_STREAM);

        let result = bridge.read_async(&path!("users/123")).await.unwrap();
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().as_bytes(),
            Some(&Bytes::from_static(b"hello"))
        );
    }

    #[tokio::test]
    async fn async_ll_to_core_write() {
        let ll = TestAsyncLLStore::new();
        let mut bridge = AsyncLLToCore::new(ll, NoCodec, Format::OCTET_STREAM);

        let record = Record::raw(Bytes::from_static(b"data"), Format::OCTET_STREAM);
        bridge
            .write_async(&path!("test/path"), record)
            .await
            .unwrap();

        // Verify it was written
        let key = vec![b"test".to_vec(), b"path".to_vec()];
        assert!(bridge.inner().data.contains_key(&key));
    }

    #[tokio::test]
    async fn async_core_to_ll_read() {
        let mut core = TestAsyncCoreStore::new();
        core.data.insert(
            path!("users/123"),
            Record::raw(Bytes::from_static(b"hello"), Format::OCTET_STREAM),
        );

        let mut bridge = AsyncCoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge.ll_read_async(&[b"users", b"123"]).await.unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"hello")));
    }

    #[tokio::test]
    async fn async_core_to_ll_write() {
        let core = TestAsyncCoreStore::new();
        let mut bridge = AsyncCoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        bridge
            .ll_write_async(&[b"test", b"path"], Bytes::from_static(b"data"))
            .await
            .unwrap();

        // Verify it was written
        assert!(bridge.inner().data.contains_key(&path!("test/path")));
    }

    #[tokio::test]
    async fn async_invalid_utf8_path_rejected() {
        let core = TestAsyncCoreStore::new();
        let mut bridge = AsyncCoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        // Invalid UTF-8 sequence
        let result = bridge.ll_read_async(&[&[0xFF, 0xFE]]).await;
        assert!(matches!(result, Err(LLError::Protocol { .. })));
    }
}
