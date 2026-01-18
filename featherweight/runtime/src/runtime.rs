//! Runtime coordinator for managing Blocks.
//!
//! The Runtime is responsible for:
//! - Creating and managing Block lifecycles
//! - Coordinating inter-Block store mounting
//! - Providing the execution environment for Blocks

use std::collections::BTreeMap;
use std::sync::Arc;

use structfs_core_store::{Error as StoreError, Path, Reader, Record, Writer};
use tokio::sync::Mutex;

use crate::block::{
    Block, BlockContext, BlockHandle, BlockId, BlockState, ErasedStore, ExportedStore,
};
use crate::error::{Result, RuntimeError};

/// Configuration for the Featherweight runtime.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Maximum number of concurrent Blocks.
    pub max_blocks: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self { max_blocks: 1024 }
    }
}

/// Registered Block with its handle and exports.
struct RegisteredBlock {
    handle: BlockHandle,
    exports: BTreeMap<String, ExportedStore>,
}

/// The Featherweight runtime.
///
/// The Runtime manages the lifecycle of Blocks and coordinates
/// inter-Block communication through store mounting.
///
/// # Example
///
/// ```ignore
/// let mut runtime = Runtime::new(RuntimeConfig::default());
///
/// // Spawn a Block
/// let handle = runtime.spawn(my_block, my_store).await?;
///
/// // Check Block state
/// assert_eq!(handle.state().await, BlockState::Running);
/// ```
pub struct Runtime {
    /// Runtime configuration.
    config: RuntimeConfig,

    /// Registered Blocks by ID.
    blocks: BTreeMap<BlockId, RegisteredBlock>,
}

impl Runtime {
    /// Create a new runtime with the given configuration.
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            blocks: BTreeMap::new(),
        }
    }

    /// Spawn a Block with the given root store.
    ///
    /// The Block will be started in a new tokio task. The returned
    /// handle can be used to monitor and control the Block.
    pub async fn spawn<B, S>(&mut self, mut block: B, root: S) -> Result<BlockHandle>
    where
        B: Block<S> + 'static,
        S: Send + 'static,
    {
        if self.blocks.len() >= self.config.max_blocks {
            return Err(RuntimeError::Io(std::io::Error::other(
                "maximum blocks reached",
            )));
        }

        let id = BlockId::new();
        let handle = BlockHandle::new(id);
        let ctx = BlockContext::new(id, root);

        // Register the Block
        self.blocks.insert(
            id,
            RegisteredBlock {
                handle: BlockHandle::new(id),
                exports: BTreeMap::new(),
            },
        );

        // Clone handle for the task
        let task_handle = BlockHandle::new(id);

        // Spawn the Block in a new task
        tokio::spawn(async move {
            task_handle.set_state(BlockState::Running).await;

            match block.run(ctx).await {
                Ok(()) => {
                    task_handle.set_state(BlockState::Stopped).await;
                }
                Err(_) => {
                    task_handle.set_state(BlockState::Failed).await;
                }
            }
        });

        handle.set_state(BlockState::Running).await;
        Ok(handle)
    }

    /// Register an export from a Block.
    ///
    /// This makes a store available for other Blocks to mount.
    pub fn register_export<S: Reader + Writer + Send + 'static>(
        &mut self,
        block_id: BlockId,
        name: &str,
        store: S,
    ) -> Result<()> {
        let block = self
            .blocks
            .get_mut(&block_id)
            .ok_or(RuntimeError::BlockNotFound(block_id.as_uuid()))?;

        block.exports.insert(
            name.to_string(),
            Arc::new(Mutex::new(Box::new(store) as Box<dyn ErasedStore>)),
        );
        Ok(())
    }

    /// Get an exported store from a Block.
    ///
    /// Returns a clone of the Arc to the store, which can be mounted
    /// in another Block's root.
    pub fn get_export(&self, block_id: BlockId, name: &str) -> Result<ExportedStore> {
        let block = self
            .blocks
            .get(&block_id)
            .ok_or(RuntimeError::BlockNotFound(block_id.as_uuid()))?;

        block
            .exports
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::ExportNotFound(name.to_string()))
    }

    /// List all Block IDs.
    pub fn blocks(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.blocks.keys().copied()
    }

    /// Get a Block's handle by ID.
    pub fn get_handle(&self, id: BlockId) -> Option<&BlockHandle> {
        self.blocks.get(&id).map(|b| &b.handle)
    }

    /// Get the number of registered Blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

/// Adapter to make a shared store (ExportedStore) usable as a Reader + Writer.
///
/// This is used when mounting inter-Block exports.
pub struct SharedStoreAdapter {
    inner: ExportedStore,
}

impl SharedStoreAdapter {
    /// Create a new adapter wrapping a shared store.
    pub fn new(store: ExportedStore) -> Self {
        Self { inner: store }
    }
}

impl Reader for SharedStoreAdapter {
    fn read(&mut self, path: &Path) -> std::result::Result<Option<Record>, StoreError> {
        // Block on the mutex - this is a sync interface over async storage
        let mut guard = self.inner.blocking_lock();
        guard.read(path)
    }
}

impl Writer for SharedStoreAdapter {
    fn write(&mut self, path: &Path, record: Record) -> std::result::Result<Path, StoreError> {
        let mut guard = self.inner.blocking_lock();
        guard.write(path, record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use structfs_core_store::{NoCodec, Value};

    #[test]
    fn runtime_config_default() {
        let config = RuntimeConfig::default();
        assert_eq!(config.max_blocks, 1024);
    }

    #[test]
    fn runtime_new() {
        let runtime = Runtime::new(RuntimeConfig::default());
        assert_eq!(runtime.block_count(), 0);
    }

    // A simple test Block implementation
    struct TestBlock {
        success: bool,
    }

    #[async_trait]
    impl crate::block::Block<()> for TestBlock {
        async fn run(&mut self, _ctx: BlockContext<()>) -> crate::error::Result<()> {
            if self.success {
                Ok(())
            } else {
                Err(crate::error::RuntimeError::Io(std::io::Error::other(
                    "test failure",
                )))
            }
        }
    }

    #[tokio::test]
    async fn runtime_spawn_block() {
        let mut runtime = Runtime::new(RuntimeConfig::default());
        let block = TestBlock { success: true };

        let handle = runtime.spawn(block, ()).await.unwrap();
        assert_eq!(handle.state().await, BlockState::Running);
        assert_eq!(runtime.block_count(), 1);
    }

    #[tokio::test]
    async fn runtime_spawn_max_blocks() {
        let config = RuntimeConfig { max_blocks: 1 };
        let mut runtime = Runtime::new(config);

        // First spawn succeeds
        let block1 = TestBlock { success: true };
        let _handle = runtime.spawn(block1, ()).await.unwrap();

        // Second spawn fails
        let block2 = TestBlock { success: true };
        let result = runtime.spawn(block2, ()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn runtime_blocks_iterator() {
        let mut runtime = Runtime::new(RuntimeConfig::default());
        let block = TestBlock { success: true };
        runtime.spawn(block, ()).await.unwrap();

        let ids: Vec<_> = runtime.blocks().collect();
        assert_eq!(ids.len(), 1);
    }

    #[tokio::test]
    async fn runtime_get_handle() {
        let mut runtime = Runtime::new(RuntimeConfig::default());
        let block = TestBlock { success: true };
        let handle = runtime.spawn(block, ()).await.unwrap();

        let retrieved = runtime.get_handle(handle.id);
        assert!(retrieved.is_some());

        // Non-existent block
        let fake_id = BlockId::new();
        assert!(runtime.get_handle(fake_id).is_none());
    }

    #[tokio::test]
    async fn runtime_register_and_get_export() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(&mut self, _path: &Path) -> std::result::Result<Option<Record>, StoreError> {
                Ok(Some(Record::parsed(Value::Integer(42))))
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

        let mut runtime = Runtime::new(RuntimeConfig::default());
        let block = TestBlock { success: true };
        let handle = runtime.spawn(block, ()).await.unwrap();

        // Register an export
        runtime
            .register_export(handle.id, "myexport", TestStore)
            .unwrap();

        // Get the export
        let export = runtime.get_export(handle.id, "myexport").unwrap();
        let mut guard = export.lock().await;
        let result = guard.read(&Path::parse("test").unwrap());
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn runtime_get_export_not_found() {
        let mut runtime = Runtime::new(RuntimeConfig::default());
        let block = TestBlock { success: true };
        let handle = runtime.spawn(block, ()).await.unwrap();

        // Export doesn't exist
        let result = runtime.get_export(handle.id, "nonexistent");
        assert!(matches!(
            result,
            Err(crate::error::RuntimeError::ExportNotFound(_))
        ));
    }

    #[tokio::test]
    async fn runtime_export_block_not_found() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(&mut self, _path: &Path) -> std::result::Result<Option<Record>, StoreError> {
                Ok(None)
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

        let mut runtime = Runtime::new(RuntimeConfig::default());

        // Block doesn't exist
        let fake_id = BlockId::new();
        let result = runtime.register_export(fake_id, "export", TestStore);
        assert!(matches!(
            result,
            Err(crate::error::RuntimeError::BlockNotFound(_))
        ));

        // Get export for non-existent block
        let result = runtime.get_export(fake_id, "export");
        assert!(matches!(
            result,
            Err(crate::error::RuntimeError::BlockNotFound(_))
        ));
    }

    #[test]
    fn shared_store_adapter() {
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
                self.value = Some(record.into_value(&NoCodec)?);
                Ok(path.clone())
            }
        }

        // Create a shared store
        let store: Box<dyn ErasedStore> = Box::new(TestStore { value: None });
        let shared: ExportedStore = Arc::new(Mutex::new(store));

        // Wrap in adapter
        let mut adapter = SharedStoreAdapter::new(shared);

        // Write through adapter
        let path = Path::parse("data").unwrap();
        let result = Writer::write(
            &mut adapter,
            &path,
            Record::parsed(Value::String("hello".to_string())),
        );
        assert!(result.is_ok());

        // Read back through adapter
        let result = Reader::read(&mut adapter, &path);
        match result {
            Ok(Some(r)) => {
                let value: Value = r.into_value(&NoCodec).unwrap();
                assert_eq!(value, Value::String("hello".to_string()));
            }
            _ => panic!("Expected Ok(Some(Record))"),
        }
    }
}
