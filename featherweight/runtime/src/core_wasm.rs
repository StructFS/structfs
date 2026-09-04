//! Host side of the core-wasm binding (`isotope/spec/11-core-wasm-binding.md`).
//!
//! Plain wasm core modules import two functions from the `structfs`
//! module and export `memory`, `block_alloc`, `manifest`, and `run`.
//! No component tooling is involved: `cargo build --target
//! wasm32-unknown-unknown` output (or hand-written wat) runs directly.
//!
//! Result delivery: the host calls the guest's `block_alloc`, copies the
//! payload into the returned buffer, and stores `{ptr, len}` (two
//! little-endian u32s) at the caller-provided ret pointer. Calls are
//! stateless; the typed error taxonomy crosses as negative status codes.

use structfs_core_store::{Codec, Error as StoreError, Format, Path, Reader, Record, Writer};
use wasmtime::{Caller, Engine, Extern, Linker, Module, Store, TypedFunc};

use crate::block::BlockId;
use crate::error::{Result, RuntimeError};

/// Spec 11 status codes.
mod status {
    pub const OK: i32 = 0;
    pub const ABSENT: i32 = 1;
    pub const NOT_FOUND: i32 = -1;
    pub const PERMISSION_DENIED: i32 = -2;
    pub const CONFLICT: i32 = -3;
    pub const OVERLOADED: i32 = -4;
    pub const DEADLINE_EXCEEDED: i32 = -5;
    pub const CANCELLED: i32 = -6;
    pub const INVALID_PATH: i32 = -7;
    pub const RESOURCE_LIMIT: i32 = -8;
    pub const OTHER: i32 = -9;
}

/// Map a typed store error onto a spec 11 status code.
fn status_of(error: &StoreError) -> i32 {
    match error {
        StoreError::NotFound { .. } | StoreError::NoRoute { .. } => status::NOT_FOUND,
        StoreError::PermissionDenied { .. } => status::PERMISSION_DENIED,
        StoreError::Conflict { .. } => status::CONFLICT,
        StoreError::Overloaded { .. } => status::OVERLOADED,
        StoreError::DeadlineExceeded { .. } => status::DEADLINE_EXCEEDED,
        StoreError::Cancelled { .. } => status::CANCELLED,
        StoreError::Path(_) => status::INVALID_PATH,
        StoreError::ResourceLimit { .. } => status::RESOURCE_LIMIT,
        _ => status::OTHER,
    }
}

/// Per-instance host state: the block's namespace plus its codec.
struct CoreState<S, C> {
    store: S,
    codec: C,
    format: Format,
}

/// Deliver payload bytes to the guest: allocate via `block_alloc`, copy,
/// and fill the ret record. Errors here are guest-contract violations
/// and surface as wasmtime errors (traps).
fn deliver<S: Send, C: Send>(
    caller: &mut Caller<'_, CoreState<S, C>>,
    ret_ptr: i32,
    payload: &[u8],
) -> std::result::Result<(), wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| wasmtime::Error::msg("guest exports no memory"))?;

    let ptr = if payload.is_empty() {
        0u32
    } else {
        let alloc: TypedFunc<i32, i32> = caller
            .get_export("block_alloc")
            .and_then(Extern::into_func)
            .ok_or_else(|| wasmtime::Error::msg("guest exports no block_alloc"))?
            .typed(&mut *caller)?;
        let ptr = alloc.call(&mut *caller, payload.len() as i32)?;
        memory.write(&mut *caller, ptr as usize, payload)?;
        ptr as u32
    };

    let mut record = [0u8; 8];
    record[..4].copy_from_slice(&ptr.to_le_bytes());
    record[4..].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    memory.write(&mut *caller, ret_ptr as usize, &record)?;
    Ok(())
}

/// Zero the ret record (absent reads).
fn deliver_none<S: Send, C: Send>(
    caller: &mut Caller<'_, CoreState<S, C>>,
    ret_ptr: i32,
) -> std::result::Result<(), wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| wasmtime::Error::msg("guest exports no memory"))?;
    memory.write(&mut *caller, ret_ptr as usize, &[0u8; 8])?;
    Ok(())
}

/// Read guest memory into a Vec.
fn read_guest<S: Send, C: Send>(
    caller: &mut Caller<'_, CoreState<S, C>>,
    ptr: i32,
    len: i32,
) -> std::result::Result<Vec<u8>, wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| wasmtime::Error::msg("guest exports no memory"))?;
    let mut buffer = vec![0u8; len as usize];
    memory.read(&caller, ptr as usize, &mut buffer)?;
    Ok(buffer)
}

fn parse_path(bytes: &[u8]) -> std::result::Result<Path, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("path is not UTF-8: {e}"))?;
    Path::parse(text).map_err(|e| e.to_string())
}

/// A block in the core-wasm binding.
pub struct CoreWasmBlock {
    module_bytes: Vec<u8>,
}

impl CoreWasmBlock {
    /// Wrap core-module bytes (or wat text — wasmtime accepts both).
    pub fn new(module_bytes: Vec<u8>) -> Self {
        Self { module_bytes }
    }

    /// Load from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self::new(std::fs::read(path)?))
    }

    fn linker<S, C>(engine: &Engine) -> Result<Linker<CoreState<S, C>>>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let mut linker: Linker<CoreState<S, C>> = Linker::new(engine);

        linker
            .func_wrap(
                "structfs",
                "read",
                |mut caller: Caller<'_, CoreState<S, C>>,
                 path_ptr: i32,
                 path_len: i32,
                 ret_ptr: i32|
                 -> std::result::Result<i32, wasmtime::Error> {
                    let path_bytes = read_guest(&mut caller, path_ptr, path_len)?;
                    let path = match parse_path(&path_bytes) {
                        Ok(path) => path,
                        Err(message) => {
                            deliver(&mut caller, ret_ptr, message.as_bytes())?;
                            return Ok(status::INVALID_PATH);
                        }
                    };
                    let state = caller.data_mut();
                    let format = state.format.clone();
                    match state.store.read(&path) {
                        Ok(None) => {
                            deliver_none(&mut caller, ret_ptr)?;
                            Ok(status::ABSENT)
                        }
                        Ok(Some(record)) => {
                            let state = caller.data_mut();
                            match record.into_bytes(&state.codec, &format) {
                                Ok(bytes) => {
                                    let bytes = bytes.to_vec();
                                    deliver(&mut caller, ret_ptr, &bytes)?;
                                    Ok(status::OK)
                                }
                                Err(e) => {
                                    let code = status_of(&e);
                                    deliver(&mut caller, ret_ptr, e.to_string().as_bytes())?;
                                    Ok(code)
                                }
                            }
                        }
                        Err(e) => {
                            let code = status_of(&e);
                            deliver(&mut caller, ret_ptr, e.to_string().as_bytes())?;
                            Ok(code)
                        }
                    }
                },
            )
            .map_err(|e| RuntimeError::wasm("linker", e))?;

        linker
            .func_wrap(
                "structfs",
                "write",
                |mut caller: Caller<'_, CoreState<S, C>>,
                 path_ptr: i32,
                 path_len: i32,
                 data_ptr: i32,
                 data_len: i32,
                 ret_ptr: i32|
                 -> std::result::Result<i32, wasmtime::Error> {
                    let path_bytes = read_guest(&mut caller, path_ptr, path_len)?;
                    let path = match parse_path(&path_bytes) {
                        Ok(path) => path,
                        Err(message) => {
                            deliver(&mut caller, ret_ptr, message.as_bytes())?;
                            return Ok(status::INVALID_PATH);
                        }
                    };
                    let data = read_guest(&mut caller, data_ptr, data_len)?;
                    let state = caller.data_mut();
                    // Decode into a parsed record here: stores receive
                    // Values, the boundary carries bytes.
                    let value = match state
                        .codec
                        .decode(&bytes::Bytes::from(data), &state.format.clone())
                    {
                        Ok(value) => value,
                        Err(e) => {
                            let code = status_of(&e);
                            deliver(&mut caller, ret_ptr, e.to_string().as_bytes())?;
                            return Ok(code);
                        }
                    };
                    let state = caller.data_mut();
                    match state.store.write(&path, Record::parsed(value)) {
                        Ok(result_path) => {
                            let text = result_path.to_string();
                            deliver(&mut caller, ret_ptr, text.as_bytes())?;
                            Ok(status::OK)
                        }
                        Err(e) => {
                            let code = status_of(&e);
                            deliver(&mut caller, ret_ptr, e.to_string().as_bytes())?;
                            Ok(code)
                        }
                    }
                },
            )
            .map_err(|e| RuntimeError::wasm("linker", e))?;

        Ok(linker)
    }

    fn instantiate<S, C>(
        &self,
        state: CoreState<S, C>,
    ) -> Result<(Store<CoreState<S, C>>, wasmtime::Instance)>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let engine = Engine::default();
        let module = Module::new(&engine, &self.module_bytes)
            .map_err(|e| RuntimeError::wasm("module", e))?;
        let linker = Self::linker::<S, C>(&engine)?;
        let mut store = Store::new(&engine, state);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| RuntimeError::wasm("instantiate", e))?;
        Ok((store, instance))
    }

    /// Read the ret record and copy the payload out of guest memory.
    fn take_ret<S, C>(
        store: &mut Store<CoreState<S, C>>,
        instance: &wasmtime::Instance,
        ret_ptr: usize,
    ) -> Result<Vec<u8>>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let memory = instance
            .get_memory(&mut *store, "memory")
            .ok_or_else(|| RuntimeError::wasm("memory", "guest exports no memory"))?;
        let mut record = [0u8; 8];
        memory
            .read(&mut *store, ret_ptr, &mut record)
            .map_err(|e| RuntimeError::wasm("ret", e))?;
        let ptr = u32::from_le_bytes(record[..4].try_into().unwrap()) as usize;
        let len = u32::from_le_bytes(record[4..].try_into().unwrap()) as usize;
        let mut payload = vec![0u8; len];
        memory
            .read(&mut *store, ptr, &mut payload)
            .map_err(|e| RuntimeError::wasm("ret", e))?;
        Ok(payload)
    }

    /// Fixed scratch address for host-driven calls' ret records: the
    /// guest's `block_alloc` provides it, keeping the host out of the
    /// guest's memory layout.
    fn alloc_ret<S, C>(
        store: &mut Store<CoreState<S, C>>,
        instance: &wasmtime::Instance,
    ) -> Result<usize>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut *store, "block_alloc")
            .map_err(|e| RuntimeError::wasm("block_alloc", e))?;
        let ptr = alloc
            .call(&mut *store, 8)
            .map_err(|e| RuntimeError::wasm("block_alloc", e))?;
        Ok(ptr as usize)
    }

    /// Retrieve the manifest (spec 01), pre-wiring.
    pub fn manifest(&self) -> Result<Vec<u8>> {
        use structfs_core_store::NoCodec;
        let state = CoreState {
            store: crate::wasm_block::NoOpStore,
            codec: NoCodec,
            format: Format::OCTET_STREAM,
        };
        let (mut store, instance) = self.instantiate(state)?;
        let ret_ptr = Self::alloc_ret(&mut store, &instance)?;
        let manifest = instance
            .get_typed_func::<i32, i32>(&mut store, "manifest")
            .map_err(|e| RuntimeError::wasm("manifest", e))?;
        let code = manifest
            .call(&mut store, ret_ptr as i32)
            .map_err(|e| RuntimeError::wasm("manifest", e))?;
        if code != status::OK {
            return Err(RuntimeError::Manifest(format!(
                "guest manifest returned status {code}"
            )));
        }
        Self::take_ret(&mut store, &instance, ret_ptr)
    }

    /// Run the block over its namespace with the declared codec/format.
    ///
    /// Returns the guest's exit code. Per spec 11 the code is advisory:
    /// a `shutdown/complete {code}` the block wrote takes precedence,
    /// which the runtime enforces when recording the outcome.
    pub fn run<S, C>(&self, _id: BlockId, root: S, codec: C, format: Format) -> Result<i32>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let state = CoreState {
            store: root,
            codec,
            format,
        };
        let (mut store, instance) = self.instantiate(state)?;
        let run = instance
            .get_typed_func::<(), i32>(&mut store, "run")
            .map_err(|e| RuntimeError::wasm("run", e))?;
        run.call(&mut store, ())
            .map_err(|e| RuntimeError::wasm("run", e))
    }
}

/// Whether wasm bytes are a component (layer 1) rather than a core
/// module — used to pick between this binding and the component one.
pub fn is_component(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[..4] == b"\0asm" && bytes[6] == 0x01
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, MemoryStore, Value};
    use structfs_serde_store::JsonCodec;

    /// A complete spec 11 guest, hand-written in wat (~40 lines): a bump
    /// allocator, a static manifest, and a run() that reads `input`,
    /// verifies `missing` is absent, and echoes the data to `output`.
    /// This is the "an SDK is an afternoon" claim, demonstrated in the
    /// least ergonomic language available.
    const ECHO_GUEST: &str = r#"
        (module
          (import "structfs" "read"
            (func $read (param i32 i32 i32) (result i32)))
          (import "structfs" "write"
            (func $write (param i32 i32 i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 4096))
          (func (export "block_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $len)))
            (local.get $ptr))
          (data (i32.const 1040) "input")
          (data (i32.const 1056) "missing")
          (data (i32.const 1072) "output")
          (data (i32.const 1088)
            "{\"name\":\"wat-echo\",\"serialization\":\"application/json\"}")
          (func (export "manifest") (param $ret i32) (result i32)
            (i32.store (local.get $ret) (i32.const 1088))
            (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 54))
            (i32.const 0))
          (func (export "run") (result i32)
            (local $st i32)
            ;; read "input" -> ret record at 1024
            (local.set $st
              (call $read (i32.const 1040) (i32.const 5) (i32.const 1024)))
            (if (i32.ne (local.get $st) (i32.const 0))
              (then (return (i32.const 1))))
            ;; read "missing" -> must be status 1 (absent)
            (local.set $st
              (call $read (i32.const 1056) (i32.const 7) (i32.const 1032)))
            (if (i32.ne (local.get $st) (i32.const 1))
              (then (return (i32.const 2))))
            ;; write the input bytes to "output"
            (local.set $st
              (call $write (i32.const 1072) (i32.const 6)
                (i32.load (i32.const 1024)) (i32.load (i32.const 1028))
                (i32.const 1032)))
            (if (i32.ne (local.get $st) (i32.const 0))
              (then (return (i32.const 3))))
            (i32.const 0)))
    "#;

    /// A guest asserting that the typed error taxonomy crosses the
    /// boundary: reading an unwired path must be status -2
    /// (permission denied / ENOTCAPABLE).
    const DENIED_GUEST: &str = r#"
        (module
          (import "structfs" "read"
            (func $read (param i32 i32 i32) (result i32)))
          (memory (export "memory") 1)
          (global $bump (mut i32) (i32.const 4096))
          (func (export "block_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (global.get $bump))
            (global.set $bump (i32.add (global.get $bump) (local.get $len)))
            (local.get $ptr))
          (data (i32.const 1040) "secret")
          (data (i32.const 1088) "{\"serialization\":\"application/json\"}")
          (func (export "manifest") (param $ret i32) (result i32)
            (i32.store (local.get $ret) (i32.const 1088))
            (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 36))
            (i32.const 0))
          (func (export "run") (result i32)
            (if (i32.ne
                  (call $read (i32.const 1040) (i32.const 6) (i32.const 1024))
                  (i32.const -2))
              (then (return (i32.const 1))))
            (i32.const 0)))
    "#;

    /// Denies everything, like an unwired namespace.
    struct DenyStore;

    impl Reader for DenyStore {
        fn read(&mut self, from: &Path) -> std::result::Result<Option<Record>, StoreError> {
            Err(StoreError::permission_denied(format!("not wired: {from}")))
        }
    }

    impl Writer for DenyStore {
        fn write(&mut self, to: &Path, _data: Record) -> std::result::Result<Path, StoreError> {
            Err(StoreError::permission_denied(format!("not wired: {to}")))
        }
    }

    #[test]
    fn manifest_crosses_the_boundary() {
        let block = CoreWasmBlock::new(ECHO_GUEST.as_bytes().to_vec());
        let manifest = block.manifest().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        assert_eq!(json["name"], "wat-echo");
        assert_eq!(json["serialization"], "application/json");
    }

    #[test]
    fn wat_guest_echoes_through_the_store() {
        let mut store = MemoryStore::new();
        store
            .write(
                &path!("input"),
                Record::parsed(Value::from("hello from the host")),
            )
            .unwrap();

        let block = CoreWasmBlock::new(ECHO_GUEST.as_bytes().to_vec());
        let mut store_after = {
            let shared = structfs_core_store::Shared::new(store);
            let code = block
                .run(BlockId::new(), shared.clone(), JsonCodec, Format::JSON)
                .unwrap();
            assert_eq!(code, 0, "guest reported failure");
            shared
        };

        // The guest round-tripped the JSON bytes through its own memory.
        let output = store_after.read(&path!("output")).unwrap().unwrap();
        assert_eq!(output.as_value(), Some(&Value::from("hello from the host")));
    }

    #[test]
    fn typed_errors_cross_as_status_codes() {
        let block = CoreWasmBlock::new(DENIED_GUEST.as_bytes().to_vec());
        let code = block
            .run(BlockId::new(), DenyStore, JsonCodec, Format::JSON)
            .unwrap();
        assert_eq!(code, 0, "guest did not observe status -2");
    }

    #[test]
    fn component_sniffing() {
        // Core module: version 1, layer 0.
        assert!(!is_component(b"\0asm\x01\x00\x00\x00rest"));
        // Component: version 0x0d, layer 1.
        assert!(is_component(b"\0asm\x0d\x00\x01\x00rest"));
        assert!(!is_component(b"short"));
        assert!(!is_component(b"not wasm"));
    }
}
