//! WASM Block execution using Wasmtime.
//!
//! This module provides the ability to load and run Blocks compiled to
//! WebAssembly components.
//!
//! The WASM boundary is an LL-store boundary: the WIT interface speaks raw
//! bytes (`list<u8>` for data, `list<list<u8>>` for paths). The runtime wraps
//! the Block's root store in a `CoreToLL` bridge with the Block's declared
//! codec and format, so the host implementation is a thin forward to
//! `ll_read`/`ll_write`.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use structfs_core_store::{Codec, CoreToLL, Error as StoreError, Format, Reader, Writer};
use structfs_ll_store::{LLReader, LLWriter};
use wasmtime::component::{bindgen, Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};

use crate::block::BlockId;
use crate::error::{Result, RuntimeError};

// Generate bindings from the WIT file
bindgen!({
    path: "wit/world.wit",
    world: "block-world",
});

/// State held by the Wasmtime store for each Block.
pub struct WasmBlockState<S, C> {
    /// The Block's unique identifier.
    pub id: BlockId,

    /// The Block's root store, wrapped in a CoreToLL bridge.
    pub root: Arc<Mutex<CoreToLL<S, C>>>,

    /// Resource table for component model.
    pub table: ResourceTable,
}

impl<S, C> WasmBlockState<S, C> {
    /// Create a new WasmBlockState.
    pub fn new(id: BlockId, root: S, codec: C, format: Format) -> Self {
        Self {
            id,
            root: Arc::new(Mutex::new(CoreToLL::new(root, codec, format))),
            table: ResourceTable::new(),
        }
    }
}

/// Implementation of the ll-store interface for WASM Blocks.
impl<S: Reader + Writer + Send + 'static, C: Codec + Send + Sync + 'static>
    featherweight::block::ll_store::Host for WasmBlockState<S, C>
{
    fn read(&mut self, path: Vec<Vec<u8>>) -> std::result::Result<Option<Vec<u8>>, String> {
        let path_refs: Vec<&[u8]> = path.iter().map(|c| c.as_slice()).collect();

        let mut root = self.root.lock().unwrap();
        match root.ll_read(&path_refs) {
            Ok(Some(bytes)) => Ok(Some(bytes.to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(
        &mut self,
        path: Vec<Vec<u8>>,
        data: Vec<u8>,
    ) -> std::result::Result<Vec<Vec<u8>>, String> {
        let path_refs: Vec<&[u8]> = path.iter().map(|c| c.as_slice()).collect();

        let mut root = self.root.lock().unwrap();
        match root.ll_write(&path_refs, Bytes::from(data)) {
            Ok(result_path) => {
                let components: Vec<Vec<u8>> =
                    result_path.into_iter().map(|b| b.to_vec()).collect();
                Ok(components)
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// A no-op store for use during manifest retrieval.
///
/// Returns `None` for all reads and echoes the path back for writes.
/// The guest's `manifest()` function should not need store access.
struct NoOpStore;

impl Reader for NoOpStore {
    fn read(
        &mut self,
        _path: &structfs_core_store::Path,
    ) -> std::result::Result<Option<structfs_core_store::Record>, StoreError> {
        Ok(None)
    }
}

impl Writer for NoOpStore {
    fn write(
        &mut self,
        path: &structfs_core_store::Path,
        _record: structfs_core_store::Record,
    ) -> std::result::Result<structfs_core_store::Path, StoreError> {
        Ok(path.clone())
    }
}

/// A WASM Block that can be loaded and executed.
pub struct WasmBlock {
    /// The compiled WASM component bytes.
    component_bytes: Vec<u8>,
}

impl WasmBlock {
    /// Create a new WasmBlock from component bytes.
    pub fn new(component_bytes: Vec<u8>) -> Self {
        Self { component_bytes }
    }

    /// Load a WasmBlock from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(Self::new(bytes))
    }

    /// Retrieve the manifest from this WASM Block.
    ///
    /// The manifest is a JSON blob declaring the block's name, version,
    /// serialization format, and path interface. The runtime calls this
    /// **before wiring** to discover what codec the block speaks—the store
    /// bridge can't be set up without it (see `isotope/rationale/04-why-manifest.md`).
    ///
    /// Creates a minimal Wasmtime environment with a no-op store,
    /// instantiates the component, and calls the guest's `manifest()` export.
    pub fn manifest(&self) -> Result<Vec<u8>> {
        use structfs_core_store::NoCodec;

        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|e| {
            RuntimeError::Store(StoreError::store("wasmtime", "engine", e.to_string()))
        })?;

        let component = Component::new(&engine, &self.component_bytes).map_err(|e| {
            RuntimeError::Store(StoreError::store("wasmtime", "component", e.to_string()))
        })?;

        let mut linker = Linker::<WasmBlockState<NoOpStore, NoCodec>>::new(&engine);
        BlockWorld::add_to_linker::<
            WasmBlockState<NoOpStore, NoCodec>,
            wasmtime::component::HasSelf<WasmBlockState<NoOpStore, NoCodec>>,
        >(&mut linker, |state: &mut WasmBlockState<NoOpStore, NoCodec>| state)
        .map_err(|e| RuntimeError::Store(StoreError::store("wasmtime", "linker", e.to_string())))?;

        let state = WasmBlockState::new(BlockId::new(), NoOpStore, NoCodec, Format::OCTET_STREAM);
        let mut store = Store::new(&engine, state);

        let instance = BlockWorld::instantiate(&mut store, &component, &linker).map_err(|e| {
            RuntimeError::Store(StoreError::store("wasmtime", "instantiate", e.to_string()))
        })?;

        let manifest_bytes = instance
            .featherweight_block_block()
            .call_manifest(&mut store)
            .map_err(|e| {
                RuntimeError::Store(StoreError::store("wasmtime", "call_manifest", e.to_string()))
            })?;

        Ok(manifest_bytes)
    }

    /// Run this WASM Block with the given root store, codec, and format.
    ///
    /// The runtime wraps `root` in a `CoreToLL` bridge using the provided
    /// `codec` and `format`, so the WASM guest sees raw bytes in the
    /// declared serialization format.
    pub fn run<S, C>(&self, id: BlockId, root: S, codec: C, format: Format) -> Result<()>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        // Create the Wasmtime engine with component model support
        let mut config = Config::new();
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|e| {
            RuntimeError::Store(StoreError::store("wasmtime", "engine", e.to_string()))
        })?;

        // Create the component from bytes
        let component = Component::new(&engine, &self.component_bytes).map_err(|e| {
            RuntimeError::Store(StoreError::store("wasmtime", "component", e.to_string()))
        })?;

        // Create the linker and add the ll-store interface
        let mut linker = Linker::<WasmBlockState<S, C>>::new(&engine);
        BlockWorld::add_to_linker::<
            WasmBlockState<S, C>,
            wasmtime::component::HasSelf<WasmBlockState<S, C>>,
        >(&mut linker, |state: &mut WasmBlockState<S, C>| state)
        .map_err(|e| RuntimeError::Store(StoreError::store("wasmtime", "linker", e.to_string())))?;

        // Create the store with our state
        let state = WasmBlockState::new(id, root, codec, format);
        let mut store = Store::new(&engine, state);

        // Instantiate the component
        let instance = BlockWorld::instantiate(&mut store, &component, &linker).map_err(|e| {
            RuntimeError::Store(StoreError::store("wasmtime", "instantiate", e.to_string()))
        })?;

        // Call the block's run function
        let result = instance
            .featherweight_block_block()
            .call_run(&mut store)
            .map_err(|e| {
                RuntimeError::Store(StoreError::store("wasmtime", "call_run", e.to_string()))
            })?;

        match result {
            Ok(()) => Ok(()),
            Err(msg) => Err(RuntimeError::Store(StoreError::store(
                "wasm_block",
                "run",
                msg,
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{Format, NoCodec, Path, Record};

    #[test]
    fn wasm_block_state_new() {
        // Simple store for testing
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        let id = BlockId::new();
        let state = WasmBlockState::new(id, TestStore, NoCodec, Format::OCTET_STREAM);
        assert_eq!(state.id, id);
    }

    #[test]
    fn wasm_block_state_host_read_not_found() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), TestStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn wasm_block_state_host_read_found() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(Some(Record::raw(
                    Bytes::from_static(b"test value"),
                    Format::OCTET_STREAM,
                )))
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), TestStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert_eq!(result, Ok(Some(b"test value".to_vec())));
    }

    #[test]
    fn wasm_block_state_host_read_invalid_path() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), TestStore, NoCodec, Format::OCTET_STREAM);
        // Path with hyphen is invalid
        let result = state.read(vec![b"foo".to_vec(), b"bar-baz".to_vec()]);
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_write_success() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), TestStore, NoCodec, Format::OCTET_STREAM);
        let result = state.write(
            vec![b"output".to_vec(), b"test".to_vec()],
            b"hello".to_vec(),
        );
        assert_eq!(
            result,
            Ok(vec![b"output".to_vec(), b"test".to_vec()])
        );
    }

    #[test]
    fn wasm_block_state_host_write_invalid_path() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for TestStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), TestStore, NoCodec, Format::OCTET_STREAM);
        // Path with hyphen is invalid
        let result = state.write(vec![b"foo".to_vec(), b"bar-baz".to_vec()], b"data".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_new() {
        let bytes = vec![0x00, 0x61, 0x73, 0x6d]; // WASM magic bytes
        let block = WasmBlock::new(bytes.clone());
        assert_eq!(block.component_bytes, bytes);
    }

    #[test]
    fn wasm_block_from_file() {
        use std::io::Write;
        // Create a temp file with some bytes
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        let bytes = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        temp.write_all(&bytes).unwrap();

        let block = WasmBlock::from_file(temp.path()).unwrap();
        assert_eq!(block.component_bytes, bytes);
    }

    #[test]
    fn wasm_block_from_file_not_found() {
        let result = WasmBlock::from_file("/nonexistent/path/to/file.wasm");
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_read_store_error() {
        struct FailingStore;
        impl Reader for FailingStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Err(structfs_core_store::Error::store(
                    "test",
                    "read",
                    "test error",
                ))
            }
        }
        impl Writer for FailingStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), FailingStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_write_store_error() {
        struct FailingStore;
        impl Reader for FailingStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(None)
            }
        }
        impl Writer for FailingStore {
            fn write(
                &mut self,
                _path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Err(structfs_core_store::Error::store(
                    "test",
                    "write",
                    "test error",
                ))
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), FailingStore, NoCodec, Format::OCTET_STREAM);
        let result = state.write(
            vec![b"output".to_vec(), b"test".to_vec()],
            b"hello".to_vec(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn wasm_block_state_host_read_codec_error() {
        struct RawBytesStore;
        impl Reader for RawBytesStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                // Return a Record with raw bytes in a different format than the bridge expects.
                // NoCodec can't transcode, so CoreToLL will fail.
                Ok(Some(Record::raw(
                    Bytes::from_static(b"\xff\xfe"),
                    Format::JSON,
                )))
            }
        }
        impl Writer for RawBytesStore {
            fn write(
                &mut self,
                path: &Path,
                _record: Record,
            ) -> std::result::Result<Path, structfs_core_store::Error> {
                Ok(path.clone())
            }
        }

        use featherweight::block::ll_store::Host;
        let mut state =
            WasmBlockState::new(BlockId::new(), RawBytesStore, NoCodec, Format::OCTET_STREAM);
        let result = state.read(vec![b"some".to_vec(), b"path".to_vec()]);
        assert!(result.is_err());
    }

    #[test]
    fn no_op_store_read_returns_none() {
        let mut store = NoOpStore;
        let path = Path::parse("some/path").unwrap();
        assert!(store.read(&path).unwrap().is_none());
    }

    #[test]
    fn no_op_store_write_echoes_path() {
        let mut store = NoOpStore;
        let path = Path::parse("some/path").unwrap();
        let record = Record::raw(Bytes::from_static(b"data"), Format::OCTET_STREAM);
        let result = store.write(&path, record).unwrap();
        assert_eq!(result, path);
    }
}
