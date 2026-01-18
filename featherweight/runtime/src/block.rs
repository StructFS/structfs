//! Block types for the Featherweight runtime.
//!
//! A Block is the fundamental unit of execution in Isotope - a pico-process
//! that runs in a WASM sandbox and interacts with the world exclusively
//! through StructFS read/write operations.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use structfs_core_store::{Reader, Writer};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::Result;

/// Unique identifier for a Block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(Uuid);

impl BlockId {
    /// Create a new random BlockId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create a BlockId from a UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Get the inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for BlockId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// State of a running Block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// Block is created but not yet started.
    Created,
    /// Block is running.
    Running,
    /// Block has stopped normally.
    Stopped,
    /// Block has failed with an error.
    Failed,
}

/// Handle to a running Block.
///
/// The handle allows monitoring and controlling a Block from outside.
#[derive(Debug)]
pub struct BlockHandle {
    /// The Block's unique identifier.
    pub id: BlockId,

    /// Current state of the Block.
    state: Arc<Mutex<BlockState>>,
}

impl BlockHandle {
    /// Create a new handle for a Block.
    pub fn new(id: BlockId) -> Self {
        Self {
            id,
            state: Arc::new(Mutex::new(BlockState::Created)),
        }
    }

    /// Get the current state of the Block.
    pub async fn state(&self) -> BlockState {
        *self.state.lock().await
    }

    /// Set the Block's state.
    pub(crate) async fn set_state(&self, state: BlockState) {
        *self.state.lock().await = state;
    }
}

/// Context provided to a Block during execution.
///
/// The BlockContext is the Block's window into the world. It provides
/// access to the Block's root store, where all data access happens.
pub struct BlockContext<S> {
    /// The Block's unique identifier.
    pub id: BlockId,

    /// The Block's root store.
    ///
    /// This store is the Block's entire view of the world. All file access,
    /// network communication, and inter-Block messaging happens through
    /// read/write operations on this store.
    pub root: S,

    /// Stores exported by this Block for other Blocks to mount.
    exports: BTreeMap<String, ExportedStore>,
}

/// A type-erased exported store.
pub type ExportedStore = Arc<Mutex<Box<dyn ErasedStore>>>;

/// Type-erased store trait for exports.
pub trait ErasedStore: Send + Sync {
    /// Read from the store.
    fn read(
        &mut self,
        path: &structfs_core_store::Path,
    ) -> std::result::Result<Option<structfs_core_store::Record>, structfs_core_store::Error>;

    /// Write to the store.
    fn write(
        &mut self,
        path: &structfs_core_store::Path,
        record: structfs_core_store::Record,
    ) -> std::result::Result<structfs_core_store::Path, structfs_core_store::Error>;
}

impl<T: Reader + Writer + Send + Sync> ErasedStore for T {
    fn read(
        &mut self,
        path: &structfs_core_store::Path,
    ) -> std::result::Result<Option<structfs_core_store::Record>, structfs_core_store::Error> {
        Reader::read(self, path)
    }

    fn write(
        &mut self,
        path: &structfs_core_store::Path,
        record: structfs_core_store::Record,
    ) -> std::result::Result<structfs_core_store::Path, structfs_core_store::Error> {
        Writer::write(self, path, record)
    }
}

impl<S> BlockContext<S> {
    /// Create a new BlockContext.
    pub fn new(id: BlockId, root: S) -> Self {
        Self {
            id,
            root,
            exports: BTreeMap::new(),
        }
    }

    /// Export a store for other Blocks to mount.
    ///
    /// The store will be available at the given name for other Blocks
    /// to import using `Runtime::mount_inter_block`.
    pub fn export<T: Reader + Writer + Send + 'static>(&mut self, name: &str, store: T) {
        self.exports.insert(
            name.to_string(),
            Arc::new(Mutex::new(Box::new(store) as Box<dyn ErasedStore>)),
        );
    }

    /// Get an exported store by name.
    pub fn get_export(&self, name: &str) -> Option<ExportedStore> {
        self.exports.get(name).cloned()
    }

    /// List all exported store names.
    pub fn exports(&self) -> impl Iterator<Item = &str> {
        self.exports.keys().map(|s| s.as_str())
    }
}

/// A Block - the fundamental execution unit in Isotope.
///
/// Blocks are pico-processes that run in WASM sandboxes. Each Block:
/// - Has a unique ID
/// - Sees only its root store (everything is mounted there)
/// - Can export stores for other Blocks to consume
/// - Communicates with other Blocks only through StructFS operations
///
/// This trait defines the interface that all Block implementations must provide.
/// The type parameter S specifies the root store type.
#[async_trait]
pub trait Block<S: Send + 'static>: Send {
    /// Run the Block to completion.
    ///
    /// The Block receives a context providing access to its root store
    /// and export capabilities. The Block should perform its work by
    /// reading and writing paths in the root store.
    async fn run(&mut self, ctx: BlockContext<S>) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{Error as StoreError, Path, Reader, Record, Value, Writer};

    #[test]
    fn block_id_display() {
        let id = BlockId::new();
        let s = format!("{}", id);
        assert_eq!(s.len(), 36); // UUID format
    }

    #[test]
    fn block_id_default() {
        let id1 = BlockId::default();
        let id2 = BlockId::default();
        assert_ne!(id1, id2);
    }

    #[test]
    fn block_id_from_uuid() {
        let uuid = uuid::Uuid::new_v4();
        let id = BlockId::from_uuid(uuid);
        assert_eq!(id.as_uuid(), uuid);
    }

    #[tokio::test]
    async fn block_handle_state() {
        let handle = BlockHandle::new(BlockId::new());
        assert_eq!(handle.state().await, BlockState::Created);

        handle.set_state(BlockState::Running).await;
        assert_eq!(handle.state().await, BlockState::Running);
    }

    #[test]
    fn block_context_new() {
        struct TestStore;
        let id = BlockId::new();
        let ctx = BlockContext::new(id, TestStore);
        assert_eq!(ctx.id, id);
        assert_eq!(ctx.exports().count(), 0);
    }

    #[test]
    fn block_context_export_and_get() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(&mut self, _path: &Path) -> std::result::Result<Option<Record>, StoreError> {
                Ok(Some(Record::parsed(Value::String("test".to_string()))))
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, StoreError> {
                Ok(path.clone())
            }
        }

        let id = BlockId::new();
        let mut ctx: BlockContext<()> = BlockContext::new(id, ());

        // Export a store
        ctx.export("mystore", TestStore);

        // Check exports list
        let export_names: Vec<_> = ctx.exports().collect();
        assert_eq!(export_names, vec!["mystore"]);

        // Get the export
        let export = ctx.get_export("mystore");
        assert!(export.is_some());

        // Non-existent export returns None
        assert!(ctx.get_export("nonexistent").is_none());
    }

    #[tokio::test]
    async fn erased_store_read_write() {
        struct TestStore {
            value: Option<Value>,
        }
        impl Reader for TestStore {
            fn read(&mut self, _path: &Path) -> std::result::Result<Option<Record>, StoreError> {
                Ok(self.value.clone().map(Record::parsed))
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                record: Record,
            ) -> std::result::Result<Path, StoreError> {
                self.value = Some(record.into_value(&structfs_core_store::NoCodec)?);
                Ok(path.clone())
            }
        }

        let store = TestStore { value: None };
        let erased: Box<dyn ErasedStore> = Box::new(store);
        let wrapped = Arc::new(Mutex::new(erased));

        // Write through erased store
        let path = Path::parse("test").unwrap();
        {
            let mut guard = wrapped.lock().await;
            let result = guard.write(&path, Record::parsed(Value::Integer(42)));
            assert!(result.is_ok());
        }

        // Read back
        {
            let mut guard = wrapped.lock().await;
            let result = guard.read(&path);
            match result {
                Ok(Some(r)) => {
                    let value = r.into_value(&structfs_core_store::NoCodec).unwrap();
                    assert_eq!(value, Value::Integer(42));
                }
                _ => panic!("Expected Ok(Some(Record))"),
            }
        }
    }
}
