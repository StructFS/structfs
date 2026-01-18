//! WASM Block execution using Wasmtime.
//!
//! This module provides the ability to load and run Blocks compiled to
//! WebAssembly components.

use std::sync::{Arc, Mutex};

use structfs_core_store::{Error as StoreError, NoCodec, Path, Reader, Record, Value, Writer};
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
pub struct WasmBlockState<S> {
    /// The Block's unique identifier.
    pub id: BlockId,

    /// The Block's root store.
    pub root: Arc<Mutex<S>>,

    /// Resource table for component model.
    pub table: ResourceTable,
}

impl<S> WasmBlockState<S> {
    /// Create a new WasmBlockState.
    pub fn new(id: BlockId, root: S) -> Self {
        Self {
            id,
            root: Arc::new(Mutex::new(root)),
            table: ResourceTable::new(),
        }
    }
}

/// Convert a StructFS Value to a WIT Value.
fn value_to_wit(val: &Value) -> featherweight::block::store::Value {
    use featherweight::block::store::Value as WitValue;
    match val {
        Value::Null => WitValue::ValNull,
        Value::Bool(b) => WitValue::ValBool(*b),
        Value::Integer(i) => WitValue::ValInteger(*i),
        Value::Float(f) => WitValue::ValFloat(*f),
        Value::String(s) => WitValue::ValText(s.clone()),
        // For now, convert complex types to their string representation
        Value::Bytes(b) => WitValue::ValText(format!("<bytes: {} bytes>", b.len())),
        Value::Array(a) => WitValue::ValText(format!("<array: {} items>", a.len())),
        Value::Map(m) => WitValue::ValText(format!("<map: {} keys>", m.len())),
    }
}

/// Convert a WIT Value to a StructFS Value.
fn wit_to_value(val: featherweight::block::store::Value) -> Value {
    use featherweight::block::store::Value as WitValue;
    match val {
        WitValue::ValNull => Value::Null,
        WitValue::ValBool(b) => Value::Bool(b),
        WitValue::ValInteger(i) => Value::Integer(i),
        WitValue::ValFloat(f) => Value::Float(f),
        WitValue::ValText(s) => Value::String(s),
    }
}

/// Implementation of the store interface for WASM Blocks.
impl<S: Reader + Writer + Send + 'static> featherweight::block::store::Host for WasmBlockState<S> {
    fn read(&mut self, path: String) -> featherweight::block::store::ReadResult {
        use featherweight::block::store::ReadResult;

        let path = match Path::parse(&path) {
            Ok(p) => p,
            Err(e) => return ReadResult::ReadError(format!("invalid path: {}", e)),
        };

        let mut root = self.root.lock().unwrap();
        match root.read(&path) {
            Ok(Some(record)) => match record.into_value(&NoCodec) {
                Ok(value) => ReadResult::Found(value_to_wit(&value)),
                Err(e) => ReadResult::ReadError(e.to_string()),
            },
            Ok(None) => ReadResult::NotFound,
            Err(e) => ReadResult::ReadError(e.to_string()),
        }
    }

    fn write(
        &mut self,
        path: String,
        val: featherweight::block::store::Value,
    ) -> featherweight::block::store::WriteResult {
        use featherweight::block::store::WriteResult;

        let parsed_path = match Path::parse(&path) {
            Ok(p) => p,
            Err(e) => return WriteResult::WriteError(format!("invalid path: {}", e)),
        };

        let value = wit_to_value(val);
        let record = Record::parsed(value);

        let mut root = self.root.lock().unwrap();
        match root.write(&parsed_path, record) {
            Ok(result_path) => WriteResult::Written(result_path.to_string()),
            Err(e) => WriteResult::WriteError(e.to_string()),
        }
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

    /// Run this WASM Block with the given root store.
    pub fn run<S: Reader + Writer + Send + 'static>(&self, id: BlockId, root: S) -> Result<()> {
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

        // Create the linker and add the store interface
        let mut linker = Linker::<WasmBlockState<S>>::new(&engine);
        BlockWorld::add_to_linker::<
            WasmBlockState<S>,
            wasmtime::component::HasSelf<WasmBlockState<S>>,
        >(&mut linker, |state: &mut WasmBlockState<S>| state)
        .map_err(|e| RuntimeError::Store(StoreError::store("wasmtime", "linker", e.to_string())))?;

        // Create the store with our state
        let state = WasmBlockState::new(id, root);
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
    use collection_literals::btree;

    #[test]
    fn value_conversion_roundtrip() {
        let values = vec![
            Value::Null,
            Value::Bool(true),
            Value::Integer(42),
            Value::Float(1.5),
            Value::String("hello".to_string()),
        ];

        for val in values {
            let wit = value_to_wit(&val);
            let back = wit_to_value(wit);
            assert_eq!(val, back);
        }
    }

    #[test]
    fn value_conversion_complex_types() {
        // Bytes get converted to descriptive text
        let bytes = Value::Bytes(vec![1, 2, 3, 4, 5]);
        let wit = value_to_wit(&bytes);
        assert!(matches!(
            wit,
            featherweight::block::store::Value::ValText(s) if s.contains("5 bytes")
        ));

        // Array gets converted to descriptive text
        let array = Value::Array(vec![Value::Integer(1), Value::Integer(2)]);
        let wit = value_to_wit(&array);
        assert!(matches!(
            wit,
            featherweight::block::store::Value::ValText(s) if s.contains("2 items")
        ));

        // Map gets converted to descriptive text
        let map = Value::Map(btree! {
            "a".to_string() => Value::Integer(1),
            "b".to_string() => Value::Integer(2),
            "c".to_string() => Value::Integer(3),
        });
        let wit = value_to_wit(&map);
        assert!(matches!(
            wit,
            featherweight::block::store::Value::ValText(s) if s.contains("3 keys")
        ));
    }

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
        let state = WasmBlockState::new(id, TestStore);
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), TestStore);
        let result = state.read("some/path".to_string());
        assert!(matches!(
            result,
            featherweight::block::store::ReadResult::NotFound
        ));
    }

    #[test]
    fn wasm_block_state_host_read_found() {
        struct TestStore;
        impl Reader for TestStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                Ok(Some(Record::parsed(Value::String(
                    "test value".to_string(),
                ))))
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), TestStore);
        let result = state.read("some/path".to_string());
        assert!(matches!(
            result,
            featherweight::block::store::ReadResult::Found(featherweight::block::store::Value::ValText(s)) if s == "test value"
        ));
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), TestStore);
        // Path with hyphen is invalid
        let result = state.read("foo/bar-baz".to_string());
        assert!(matches!(
            result,
            featherweight::block::store::ReadResult::ReadError(_)
        ));
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), TestStore);
        let result = state.write(
            "output/test".to_string(),
            featherweight::block::store::Value::ValText("hello".to_string()),
        );
        assert!(matches!(
            result,
            featherweight::block::store::WriteResult::Written(p) if p == "output/test"
        ));
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), TestStore);
        // Path with hyphen is invalid
        let result = state.write(
            "foo/bar-baz".to_string(),
            featherweight::block::store::Value::ValNull,
        );
        assert!(matches!(
            result,
            featherweight::block::store::WriteResult::WriteError(_)
        ));
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), FailingStore);
        let result = state.read("some/path".to_string());
        assert!(matches!(
            result,
            featherweight::block::store::ReadResult::ReadError(_)
        ));
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), FailingStore);
        let result = state.write(
            "output/test".to_string(),
            featherweight::block::store::Value::ValText("hello".to_string()),
        );
        assert!(matches!(
            result,
            featherweight::block::store::WriteResult::WriteError(_)
        ));
    }

    #[test]
    fn wasm_block_state_host_read_decode_error() {
        use structfs_core_store::Format;

        struct RawBytesStore;
        impl Reader for RawBytesStore {
            fn read(
                &mut self,
                _path: &Path,
            ) -> std::result::Result<Option<Record>, structfs_core_store::Error> {
                // Return a Record with raw bytes that can't be decoded without a codec
                Ok(Some(Record::raw(vec![0xFF, 0xFE], Format::OCTET_STREAM)))
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

        use featherweight::block::store::Host;
        let mut state = WasmBlockState::new(BlockId::new(), RawBytesStore);
        let result = state.read("some/path".to_string());
        assert!(matches!(
            result,
            featherweight::block::store::ReadResult::ReadError(_)
        ));
    }
}
