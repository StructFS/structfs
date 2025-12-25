//! Async traits for the Core layer.
//!
//! These traits are async versions of `Reader` and `Writer`, for use
//! with async runtimes like Tokio.
//!
//! Enable the `async` feature to use these traits:
//!
//! ```toml
//! [dependencies]
//! structfs-core-store = { version = "0.1", features = ["async"] }
//! ```

use async_trait::async_trait;

use crate::{Error, Path, Record};

/// Async version of `Reader`.
///
/// Read records from paths asynchronously. This is useful for I/O-bound
/// operations like network requests or async file I/O.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn AsyncReader>`.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_core_store::{AsyncReader, Record, Path, Error, path};
///
/// async fn read_user(store: &mut dyn AsyncReader) -> Result<Option<Record>, Error> {
///     store.read_async(&path!("users/123")).await
/// }
/// ```
#[async_trait]
pub trait AsyncReader: Send + Sync {
    /// Read a record from a path asynchronously.
    ///
    /// # Returns
    ///
    /// * `Ok(None)` - The path does not exist.
    /// * `Ok(Some(record))` - The record at the path.
    /// * `Err(Error)` - An error occurred.
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error>;
}

/// Async version of `Writer`.
///
/// Write records to paths asynchronously. This is useful for I/O-bound
/// operations like network requests or async file I/O.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn AsyncWriter>`.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_core_store::{AsyncWriter, Record, Path, Error, path, Value};
///
/// async fn write_user(store: &mut dyn AsyncWriter, user: Value) -> Result<Path, Error> {
///     store.write_async(&path!("users/new"), Record::parsed(user)).await
/// }
/// ```
#[async_trait]
pub trait AsyncWriter: Send + Sync {
    /// Write a record to a path asynchronously.
    ///
    /// # Returns
    ///
    /// The "result path" where the data was written. This may be:
    /// - The same as the input path
    /// - A different path (e.g., a generated ID)
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error>;
}

/// Combined async read/write at the Core level.
///
/// This is a convenience trait for stores that support both async reading
/// and writing. It is automatically implemented for any type that implements
/// both `AsyncReader` and `AsyncWriter`.
pub trait AsyncStore: AsyncReader + AsyncWriter {}
impl<T: AsyncReader + AsyncWriter> AsyncStore for T {}

// Blanket implementations for references and boxes

#[async_trait]
impl<T: AsyncReader + ?Sized> AsyncReader for &mut T {
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        (*self).read_async(from).await
    }
}

#[async_trait]
impl<T: AsyncWriter + ?Sized> AsyncWriter for &mut T {
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        (*self).write_async(to, data).await
    }
}

#[async_trait]
impl<T: AsyncReader + ?Sized> AsyncReader for Box<T> {
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.as_mut().read_async(from).await
    }
}

#[async_trait]
impl<T: AsyncWriter + ?Sized> AsyncWriter for Box<T> {
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.as_mut().write_async(to, data).await
    }
}

/// Adapter to wrap a sync store for async use.
///
/// This wraps the store in a Mutex for thread-safe access. For high-performance
/// use cases, consider implementing `AsyncReader`/`AsyncWriter` directly with
/// proper async I/O.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_core_store::{SyncToAsync, Reader, Writer};
///
/// let sync_store = MySyncStore::new();
/// let async_store = SyncToAsync::new(sync_store);
/// ```
pub struct SyncToAsync<T> {
    inner: std::sync::Arc<std::sync::Mutex<T>>,
}

impl<T> SyncToAsync<T> {
    /// Create a new adapter wrapping a sync store.
    pub fn new(inner: T) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(inner)),
        }
    }

    /// Get a reference to the inner mutex.
    pub fn inner(&self) -> &std::sync::Mutex<T> {
        &self.inner
    }
}

impl<T> Clone for SyncToAsync<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[async_trait]
impl<T: crate::Reader + Send + 'static> AsyncReader for SyncToAsync<T> {
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let path = from.clone();
        let inner = self.inner.clone();

        let mut guard = inner.lock().map_err(|_| Error::Other {
            message: "lock poisoned".into(),
        })?;

        guard.read(&path)
    }
}

#[async_trait]
impl<T: crate::Writer + Send + 'static> AsyncWriter for SyncToAsync<T> {
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let path = to.clone();
        let inner = self.inner.clone();

        let mut guard = inner.lock().map_err(|_| Error::Other {
            message: "lock poisoned".into(),
        })?;

        guard.write(&path, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Format, Value};
    use bytes::Bytes;
    use std::collections::HashMap;

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

    #[tokio::test]
    async fn async_read_write_works() {
        use crate::path;

        let mut store = TestAsyncStore::new();

        // Write
        let record = Record::raw(Bytes::from_static(b"hello"), Format::JSON);
        store
            .write_async(&path!("users/123"), record)
            .await
            .unwrap();

        // Read
        let result = store.read_async(&path!("users/123")).await.unwrap();
        assert!(result.is_some());

        // Read non-existent
        let result = store.read_async(&path!("nonexistent")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn async_with_parsed_values() {
        use crate::path;

        let mut store = TestAsyncStore::new();

        // Write a parsed value
        let value = Value::from("hello world");
        store
            .write_async(&path!("data/greeting"), Record::parsed(value))
            .await
            .unwrap();

        // Read back
        let result = store.read_async(&path!("data/greeting")).await.unwrap();
        assert!(result.is_some());
        let record = result.unwrap();
        assert!(record.is_parsed());
        assert_eq!(record.as_value(), Some(&Value::from("hello world")));
    }

    #[tokio::test]
    async fn object_safety_works() {
        use crate::path;

        let mut store = TestAsyncStore::new();
        let boxed: &mut dyn AsyncStore = &mut store;

        boxed
            .write_async(
                &path!("test"),
                Record::raw(Bytes::from_static(b"data"), Format::OCTET_STREAM),
            )
            .await
            .unwrap();

        let result = boxed.read_async(&path!("test")).await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn sync_to_async_adapter_works() {
        use crate::{path, Reader, Writer};

        // Create a sync store
        struct SyncStore {
            data: HashMap<Path, Record>,
        }

        impl Reader for SyncStore {
            fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
                Ok(self.data.get(from).cloned())
            }
        }

        impl Writer for SyncStore {
            fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
                self.data.insert(to.clone(), data);
                Ok(to.clone())
            }
        }

        let sync_store = SyncStore {
            data: HashMap::new(),
        };
        let mut async_store = SyncToAsync::new(sync_store);

        // Use async interface
        async_store
            .write_async(
                &path!("key"),
                Record::raw(Bytes::from_static(b"value"), Format::OCTET_STREAM),
            )
            .await
            .unwrap();

        let result = async_store.read_async(&path!("key")).await.unwrap();
        assert!(result.is_some());
    }
}
