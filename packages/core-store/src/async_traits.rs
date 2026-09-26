//! Async traits for the Core layer.
//!
//! These traits are async versions of `Reader` and `Writer`, for use
//! with async runtimes like Tokio.
//!
//! Enable the `async` feature to use these traits:
//!
//! ```toml
//! [dependencies]
//! structfs-core-store = { version = "0.4", features = ["async"] }
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
/// Each async operation takes the inner `Mutex` and runs the sync operation
/// **inline on the calling task** — there is no `spawn_blocking` and no
/// thread hop, so the future completes on first poll. That makes it right
/// for stores whose operations are short (in-memory, cheap computation); a
/// store that blocks on I/O will block the executor and deserves a
/// purpose-built `AsyncReader`/`AsyncWriter` implementation.
///
/// Lock poisoning is recovered from, following the crate-wide policy
/// documented on [`Shared`](crate::Shared).
///
/// # Example
///
/// ```rust,ignore
/// use structfs_core_store::{SyncToAsync, Reader, Writer};
///
/// let sync_store = MySyncStore::new();
/// let async_store = SyncToAsync::new(sync_store);
/// ```
///
/// A thin view over [`Shared`](crate::Shared): the same `Arc<Mutex<T>>`
/// handle, implementing the async traits instead of the sync ones. Build one
/// from an existing `Shared` with `From` to reach one store both ways.
pub struct SyncToAsync<T> {
    shared: crate::Shared<T>,
}

impl<T> SyncToAsync<T> {
    /// Create a new adapter wrapping a sync store.
    pub fn new(inner: T) -> Self {
        Self {
            shared: crate::Shared::new(inner),
        }
    }

    /// Get a reference to the inner mutex.
    pub fn inner(&self) -> &std::sync::Mutex<T> {
        self.shared.mutex()
    }
}

impl<T> From<crate::Shared<T>> for SyncToAsync<T> {
    fn from(shared: crate::Shared<T>) -> Self {
        Self { shared }
    }
}

impl<T> Clone for SyncToAsync<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

#[async_trait]
impl<T: crate::Reader + Send + 'static> AsyncReader for SyncToAsync<T> {
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.shared.lock().read(from)
    }
}

#[async_trait]
impl<T: crate::Writer + Send + 'static> AsyncWriter for SyncToAsync<T> {
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.shared.lock().write(to, data)
    }
}

/// Shared read capability. Construction must be prompt; acceptance and abandonment
/// semantics belong to the provider. A detached future does not imply rollback.
///
/// The method is named `read_shared` (not `read`) so a type that is both a
/// `Reader` and a `SharedReader` needs no fully-qualified calls.
pub trait SharedReader: Send + Sync {
    /// Begin a read through a shared handle; the future owns its arguments.
    fn read_shared(&self, path: Path) -> DetachedFuture<Option<Record>>;
}

/// Shared write capability with independent owned operations. Providers define
/// acceptance, ordering and what dropping an unpolled or pending future does.
///
/// The method is named `write_shared` (not `write`) for the same reason as
/// [`SharedReader::read_shared`].
pub trait SharedWriter: Send + Sync {
    /// Begin a write through a shared handle; the future owns its arguments.
    fn write_shared(&self, path: Path, record: Record) -> DetachedFuture<Path>;
}

impl<T: SharedReader + ?Sized> SharedReader for std::sync::Arc<T> {
    fn read_shared(&self, path: Path) -> DetachedFuture<Option<Record>> {
        (**self).read_shared(path)
    }
}
impl<T: SharedWriter + ?Sized> SharedWriter for std::sync::Arc<T> {
    fn write_shared(&self, path: Path, record: Record) -> DetachedFuture<Path> {
        (**self).write_shared(path, record)
    }
}
// Shared clients can be composed by every existing detached combinator without
// another wrapper or a lock held over a parked request.
impl<T: SharedReader + ?Sized> DetachedReader for std::sync::Arc<T> {
    fn read_detached(&mut self, path: &Path) -> DetachedFuture<Option<Record>> {
        (**self).read_shared(path.clone())
    }
}
impl<T: SharedWriter + ?Sized> DetachedWriter for std::sync::Arc<T> {
    fn write_detached(&mut self, path: &Path, record: Record) -> DetachedFuture<Path> {
        (**self).write_shared(path.clone(), record)
    }
}

// === Detached async traits ===

/// A boxed future that does not borrow the store that produced it.
pub type DetachedFuture<T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Error>> + Send + 'static>>;

/// Async reads whose futures are detached from the store.
///
/// `AsyncReader`'s futures borrow `&mut self`, so one in-flight read holds
/// the store's exclusive borrow for the whole operation — a long-parked
/// read stalls every other request. `DetachedReader` splits the phases:
/// the store produces the future synchronously (`&mut self`, briefly), and
/// the future resolves asynchronously with no borrow of the store, so many
/// operations can be in flight at once.
///
/// Implementors typically clone an `Arc` of shared state into the future.
pub trait DetachedReader: Send {
    /// Begin a read; the returned future resolves independently.
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>>;
}

/// Async writes whose futures are detached from the store.
///
/// See [`DetachedReader`] for the rationale.
pub trait DetachedWriter: Send {
    /// Begin a write; the returned future resolves independently.
    fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path>;
}

/// Combined detached read/write.
pub trait DetachedStore: DetachedReader + DetachedWriter {}
impl<T: DetachedReader + DetachedWriter> DetachedStore for T {}

impl<T: DetachedReader + ?Sized> DetachedReader for &mut T {
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
        (*self).read_detached(from)
    }
}

impl<T: DetachedWriter + ?Sized> DetachedWriter for &mut T {
    fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
        (*self).write_detached(to, data)
    }
}

impl<T: DetachedReader + ?Sized> DetachedReader for Box<T> {
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
        self.as_mut().read_detached(from)
    }
}

impl<T: DetachedWriter + ?Sized> DetachedWriter for Box<T> {
    fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
        self.as_mut().write_detached(to, data)
    }
}

// `Shared` already owns an `Arc<Mutex<S>>`, so it can hand out detached
// futures over a sync store: the future clones the handle and locks when
// polled. Sync store operations should be short; long-blocking sync stores
// deserve a purpose-built DetachedReader implementation.
impl<S: crate::Reader + 'static> DetachedReader for crate::Shared<S> {
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
        let shared = self.clone();
        let path = from.clone();
        Box::pin(async move { shared.lock().read(&path) })
    }
}

impl<S: crate::Writer + 'static> DetachedWriter for crate::Shared<S> {
    fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
        let shared = self.clone();
        let path = to.clone();
        Box::pin(async move { shared.lock().write(&path, data) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RawMapStore;
    use crate::{Format, Value};
    use bytes::Bytes;

    /// An async store that holds raw records verbatim: the raw-capable map
    /// store behind the sync-to-async adapter.
    type TestAsyncStore = SyncToAsync<RawMapStore>;

    fn test_async_store() -> TestAsyncStore {
        SyncToAsync::new(RawMapStore::new())
    }

    #[tokio::test]
    async fn async_read_write_works() {
        let mut store = test_async_store();

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
        let mut store = test_async_store();

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
        let mut store = test_async_store();
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
    async fn detached_futures_do_not_borrow_the_store() {
        use crate::{MemoryStore, Shared};

        let mut store = Shared::new(MemoryStore::new());

        // Start a write, then a read, while both futures are pending —
        // impossible with borrowed futures, the point of the detached traits.
        let write_fut = store.write_detached(&path!("key"), Record::parsed(Value::from("v")));
        write_fut.await.unwrap();

        let read_fut = store.read_detached(&path!("key"));
        let another_read = store.read_detached(&path!("key"));
        let (a, b) = (read_fut.await.unwrap(), another_read.await.unwrap());
        assert!(a.is_some());
        assert!(b.is_some());
    }

    #[tokio::test]
    async fn detached_object_safety() {
        use crate::{MemoryStore, Shared};

        let mut boxed: Box<dyn DetachedStore> = Box::new(Shared::new(MemoryStore::new()));
        boxed
            .write_detached(&path!("k"), Record::parsed(Value::from(1i64)))
            .await
            .unwrap();
        assert!(boxed.read_detached(&path!("k")).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn sync_to_async_adapter_works() {
        use crate::MemoryStore;

        let mut async_store = SyncToAsync::new(MemoryStore::new());

        // Use async interface
        async_store
            .write_async(&path!("key"), Record::parsed(Value::from("value")))
            .await
            .unwrap();

        let result = async_store.read_async(&path!("key")).await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn sync_to_async_recovers_from_poisoned_lock() {
        use crate::{MemoryStore, Writer};

        let store = SyncToAsync::new(MemoryStore::new());
        let poisoner = store.clone();
        let _ = std::thread::spawn(move || {
            let mut guard = poisoner.inner().lock().unwrap();
            guard
                .write(&path!("before_panic"), Record::parsed(Value::from(1i64)))
                .unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(store.inner().is_poisoned());

        let mut store = store;
        assert!(store
            .read_async(&path!("before_panic"))
            .await
            .unwrap()
            .is_some());
        store
            .write_async(&path!("after"), Record::parsed(Value::from(2i64)))
            .await
            .unwrap();
    }
}
