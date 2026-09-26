//! Async traits for the LL layer.
//!
//! These traits are async versions of `LLReader` and `LLWriter`, for use
//! with async runtimes like Tokio.
//!
//! Enable the `async` feature to use these traits:
//!
//! ```toml
//! [dependencies]
//! structfs-ll-store = { version = "0.4", features = ["async"] }
//! ```

use async_trait::async_trait;
use bytes::Bytes;

use crate::{LLError, LLPath};

/// Async version of `LLReader`.
///
/// Read bytes from a path asynchronously. This is useful for I/O-bound
/// operations like network requests or async file I/O.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn AsyncLLReader>`.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_ll_store::{AsyncLLReader, LLError};
/// use bytes::Bytes;
///
/// async fn read_user(store: &mut dyn AsyncLLReader, user_id: &str) -> Result<Option<Bytes>, LLError> {
///     store.ll_read_async(&[b"users", user_id.as_bytes()]).await
/// }
/// ```
#[async_trait]
pub trait AsyncLLReader: Send + Sync {
    /// Read raw bytes from path components asynchronously.
    ///
    /// # Arguments
    ///
    /// * `path` - A slice of byte slices representing path components.
    ///
    /// # Returns
    ///
    /// * `Ok(None)` - The path does not exist.
    /// * `Ok(Some(bytes))` - The data at the path.
    /// * `Err(LLError)` - A transport or system error occurred.
    async fn ll_read_async(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError>;
}

/// Async version of `LLWriter`.
///
/// Write bytes to a path asynchronously. This is useful for I/O-bound
/// operations like network requests or async file I/O.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn AsyncLLWriter>`.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_ll_store::{AsyncLLWriter, LLPath, LLError};
/// use bytes::Bytes;
///
/// async fn create_user(store: &mut dyn AsyncLLWriter, data: &[u8]) -> Result<LLPath, LLError> {
///     store.ll_write_async(&[b"users"], Bytes::copy_from_slice(data)).await
/// }
/// ```
#[async_trait]
pub trait AsyncLLWriter: Send + Sync {
    /// Write raw bytes to path components asynchronously.
    ///
    /// # Arguments
    ///
    /// * `path` - A slice of byte slices representing path components.
    /// * `data` - The bytes to write.
    ///
    /// # Returns
    ///
    /// The "result path" as a sequence of byte components.
    async fn ll_write_async(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError>;
}

/// Combined async read/write at the LL level.
///
/// This is a convenience trait for stores that support both async reading
/// and writing. It is automatically implemented for any type that implements
/// both `AsyncLLReader` and `AsyncLLWriter`.
pub trait AsyncLLStore: AsyncLLReader + AsyncLLWriter {}
impl<T: AsyncLLReader + AsyncLLWriter> AsyncLLStore for T {}

// Blanket implementations for references and boxes

#[async_trait]
impl<T: AsyncLLReader + ?Sized> AsyncLLReader for &mut T {
    async fn ll_read_async(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        (*self).ll_read_async(path).await
    }
}

#[async_trait]
impl<T: AsyncLLWriter + ?Sized> AsyncLLWriter for &mut T {
    async fn ll_write_async(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        (*self).ll_write_async(path, data).await
    }
}

#[async_trait]
impl<T: AsyncLLReader + ?Sized> AsyncLLReader for Box<T> {
    async fn ll_read_async(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        self.as_mut().ll_read_async(path).await
    }
}

#[async_trait]
impl<T: AsyncLLWriter + ?Sized> AsyncLLWriter for Box<T> {
    async fn ll_write_async(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        self.as_mut().ll_write_async(path, data).await
    }
}

/// Adapter to wrap a sync `LLReader`/`LLWriter` for async use.
///
/// Each async operation takes the inner `Mutex` and runs the sync operation
/// **inline on the calling task** — there is no `spawn_blocking` and no
/// thread hop. That makes it suitable for stores whose operations are short
/// (in-memory, cheap computation); a store that blocks on I/O will block
/// the executor and deserves a purpose-built async implementation.
///
/// Lock poisoning is recovered from: if a previous operation panicked while
/// holding the lock, the next operation proceeds against the store as it
/// was left (the same policy as `Shared`/`SyncToAsync` in core-store).
///
/// # Example
///
/// ```rust,ignore
/// use structfs_ll_store::{SyncToAsyncLL, LLReader};
///
/// let sync_store = MySyncStore::new();
/// let async_store = SyncToAsyncLL::new(sync_store);
/// ```
pub struct SyncToAsyncLL<T> {
    inner: std::sync::Arc<std::sync::Mutex<T>>,
}

impl<T> SyncToAsyncLL<T> {
    /// Create a new adapter wrapping a sync store.
    pub fn new(inner: T) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(inner)),
        }
    }

    /// Get a reference to the inner store.
    pub fn inner(&self) -> &std::sync::Mutex<T> {
        &self.inner
    }

    /// Lock the inner store, recovering from poisoning.
    fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<T> Clone for SyncToAsyncLL<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[async_trait]
impl<T: crate::LLReader + Send + 'static> AsyncLLReader for SyncToAsyncLL<T> {
    async fn ll_read_async(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // The borrowed components outlive the (inline, non-suspending) call.
        self.lock().ll_read(path)
    }
}

#[async_trait]
impl<T: crate::LLWriter + Send + 'static> AsyncLLWriter for SyncToAsyncLL<T> {
    async fn ll_write_async(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        self.lock().ll_write(path, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    #[tokio::test]
    async fn async_read_write_works() {
        let mut store = TestAsyncLLStore::new();

        // Write
        let path = &[b"users".as_slice(), b"123".as_slice()];
        let data = Bytes::from_static(b"hello async");
        store.ll_write_async(path, data.clone()).await.unwrap();

        // Read
        let result = store.ll_read_async(path).await.unwrap();
        assert_eq!(result, Some(data));

        // Read non-existent
        let result = store.ll_read_async(&[b"nonexistent"]).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn object_safety_works() {
        let mut store = TestAsyncLLStore::new();
        let boxed: &mut dyn AsyncLLStore = &mut store;

        boxed
            .ll_write_async(&[b"test"], Bytes::from_static(b"data"))
            .await
            .unwrap();
        let result = boxed.ll_read_async(&[b"test"]).await.unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"data")));
    }

    #[tokio::test]
    async fn sync_to_async_adapter_works() {
        use crate::traits::{LLReader, LLWriter};

        // Create a sync store
        struct SyncStore {
            data: HashMap<Vec<Vec<u8>>, Bytes>,
        }

        impl LLReader for SyncStore {
            fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
                let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
                Ok(self.data.get(&key).cloned())
            }
        }

        impl LLWriter for SyncStore {
            fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
                let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
                self.data.insert(key, data);
                Ok(path.iter().map(|c| Bytes::copy_from_slice(c)).collect())
            }
        }

        let sync_store = SyncStore {
            data: HashMap::new(),
        };
        let mut async_store = SyncToAsyncLL::new(sync_store);

        // Use async interface
        async_store
            .ll_write_async(&[b"key"], Bytes::from_static(b"value"))
            .await
            .unwrap();

        let result = async_store.ll_read_async(&[b"key"]).await.unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"value")));
    }

    #[tokio::test]
    async fn sync_to_async_recovers_from_poisoned_lock() {
        struct Store;
        impl crate::LLReader for Store {
            fn ll_read(&mut self, _: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
                Ok(Some(Bytes::from_static(b"alive")))
            }
        }
        let store = SyncToAsyncLL::new(Store);
        let poisoner = store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.inner().lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(store.inner().is_poisoned());
        let mut store = store;
        assert_eq!(
            store.ll_read_async(&[b"k"]).await.unwrap(),
            Some(Bytes::from_static(b"alive"))
        );
    }
}
