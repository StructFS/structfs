//! Host side of the core-wasm binding
//! ([spec 11](https://github.com/StructFS/structfs/blob/main/isotope/spec/11-core-wasm-binding.md)).
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

use std::sync::Arc;

use structfs_core_store::{Codec, Error as StoreError, Format, Path, Reader, Record, Writer};
use structfs_handles::CancelToken;
use wasmtime::{Caller, Engine, Extern, Linker, Module, Store, TypedFunc};

use crate::block::BlockId;
use crate::error::{Result, RuntimeError};
use crate::metering::{EpochTicker, Metering};

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
    limits: wasmtime::StoreLimits,
    usage: Option<(crate::ExecutionMeter, u64)>,
    memory: Option<wasmtime::Memory>,
    execution: Option<(crate::ExecutionPolicy, CancelToken)>,
}

fn check_host<S, C>(
    caller: &Caller<'_, CoreState<S, C>>,
) -> std::result::Result<(), wasmtime::Error> {
    if let Some((policy, cancel)) = &caller.data().execution {
        policy
            .ensure_active(cancel)
            .map_err(|e| wasmtime::Error::msg(e.to_string()))?;
    }
    Ok(())
}

fn sample_caller<S, C>(caller: &mut Caller<'_, CoreState<S, C>>) {
    if let Some((meter, initial)) = caller.data().usage.clone() {
        if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
            meter.sample_wasm(
                initial.saturating_sub(caller.get_fuel().unwrap_or(initial)),
                memory.data_size(&*caller),
            );
        }
    }
}

/// Host transfer budget, independent of the guest's linear-memory size.
const MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

fn checked_range(
    ptr: usize,
    len: usize,
    memory_len: usize,
) -> std::result::Result<std::ops::Range<usize>, wasmtime::Error> {
    if len > MAX_TRANSFER_BYTES {
        return Err(wasmtime::Error::msg("guest transfer exceeds 64 MiB limit"));
    }
    let end = ptr
        .checked_add(len)
        .filter(|end| *end <= memory_len)
        .ok_or_else(|| wasmtime::Error::msg("guest memory range out of bounds"))?;
    Ok(ptr..end)
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

    checked_range(ret_ptr as u32 as usize, 8, memory.data_size(&*caller))?;
    if payload.len() > MAX_TRANSFER_BYTES {
        return Err(wasmtime::Error::msg("guest transfer exceeds 64 MiB limit"));
    }
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

async fn deliver_async<S: Send, C: Send>(
    caller: &mut Caller<'_, CoreState<S, C>>,
    ret_ptr: i32,
    payload: &[u8],
) -> std::result::Result<(), wasmtime::Error> {
    let memory = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| wasmtime::Error::msg("guest exports no memory"))?;

    checked_range(ret_ptr as u32 as usize, 8, memory.data_size(&*caller))?;
    if payload.len() > MAX_TRANSFER_BYTES {
        return Err(wasmtime::Error::msg("guest transfer exceeds 64 MiB limit"));
    }
    let ptr = if payload.is_empty() {
        0u32
    } else {
        let alloc: TypedFunc<i32, i32> = caller
            .get_export("block_alloc")
            .and_then(Extern::into_func)
            .ok_or_else(|| wasmtime::Error::msg("guest exports no block_alloc"))?
            .typed(&mut *caller)?;
        let ptr = alloc.call_async(&mut *caller, payload.len() as i32).await?;
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
    let range = checked_range(
        ptr as u32 as usize,
        len as u32 as usize,
        memory.data_size(&*caller),
    )?;
    Ok(memory.data(&*caller)[range].to_vec())
}

fn parse_path(bytes: &[u8]) -> std::result::Result<Path, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("path is not UTF-8: {e}"))?;
    Path::parse(text).map_err(|e| e.to_string())
}

/// A no-op store for use during manifest retrieval.
///
/// Returns `None` for all reads and echoes the path back for writes.
/// The guest's `manifest()` function should not need store access.
/// Public for binding adapters, which face the same bootstrap: the
/// manifest is retrieved before any store bridge exists.
pub struct NoOpStore;

impl Reader for NoOpStore {
    fn read(&mut self, _path: &Path) -> std::result::Result<Option<Record>, StoreError> {
        Ok(None)
    }
}

impl Writer for NoOpStore {
    fn write(&mut self, path: &Path, _record: Record) -> std::result::Result<Path, StoreError> {
        Ok(path.clone())
    }
}

#[async_trait::async_trait]
impl structfs_core_store::AsyncReader for NoOpStore {
    async fn read_async(&mut self, path: &Path) -> std::result::Result<Option<Record>, StoreError> {
        self.read(path)
    }
}
#[async_trait::async_trait]
impl structfs_core_store::AsyncWriter for NoOpStore {
    async fn write_async(
        &mut self,
        path: &Path,
        record: Record,
    ) -> std::result::Result<Path, StoreError> {
        self.write(path, record)
    }
}

/// A block in the core-wasm binding.
#[derive(Clone)]
pub struct CoreWasmBlock {
    module_bytes: Vec<u8>,
    prepared: Option<(Arc<CoreWasmEngine>, Module, Vec<u8>)>,
    session: Option<Arc<CoreWasmSession>>,
}

/// Shared compilation service for fresh guest stores. Create inside Tokio and
/// keep its executor alive until all prepared artifacts and sessions finish.
/// Artifact retention/eviction belongs to the embedding host.
pub struct CoreWasmEngine {
    engine: Engine,
    _ticker: crate::metering::AsyncEpochTicker,
    compilation: Arc<tokio::sync::Semaphore>,
    memory_limit: usize,
    sessions: Arc<tokio::sync::Semaphore>,
    epoch_interval: std::time::Duration,
}

/// Reserves every guest slot needed by an assembly before any block starts.
/// Keep all artifacts bound to this reservation in a request-scoped runtime.
pub struct CoreWasmSession {
    engine: Arc<CoreWasmEngine>,
    slots: Arc<tokio::sync::Semaphore>,
    _reservation: tokio::sync::OwnedSemaphorePermit,
}

impl CoreWasmEngine {
    /// One engine and one 10 ms epoch ticker; at most `compile_parallelism`
    /// compilations run concurrently. No guest memory is shared.
    pub fn new(compile_parallelism: usize) -> Result<Arc<Self>> {
        Self::with_limits(compile_parallelism, 10_000, 64 * 1024 * 1024)
    }

    /// Bound active stores and per-store linear memory. Waiting for a store
    /// slot is cancellable; the HTTP edge must separately bound admission.
    pub fn with_limits(
        compile_parallelism: usize,
        max_sessions: usize,
        memory_limit: usize,
    ) -> Result<Arc<Self>> {
        Self::with_epoch_interval(
            compile_parallelism,
            max_sessions,
            memory_limit,
            std::time::Duration::from_millis(10),
        )
    }

    /// Engine-wide epoch cadence. Per-run policy never changes the shared ticker.
    pub fn with_epoch_interval(
        compile_parallelism: usize,
        max_sessions: usize,
        memory_limit: usize,
        epoch_interval: std::time::Duration,
    ) -> Result<Arc<Self>> {
        if epoch_interval.is_zero() {
            return Err(RuntimeError::wasm(
                "engine",
                "epoch interval must be positive",
            ));
        }
        if max_sessions == 0 || memory_limit == 0 {
            return Err(RuntimeError::wasm(
                "engine",
                "session and memory limits must be positive",
            ));
        }
        if compile_parallelism == 0 {
            return Err(RuntimeError::wasm(
                "engine",
                "compile parallelism must be positive",
            ));
        }
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        config.cranelift_nan_canonicalization(true);
        config.relaxed_simd_deterministic(true);
        let engine = Engine::new(&config).map_err(|e| RuntimeError::wasm("engine", e))?;
        tokio::runtime::Handle::try_current().map_err(|_| {
            RuntimeError::wasm("engine", "create the engine inside a live Tokio runtime")
        })?;
        let ticker = Metering {
            fuel: None,
            epoch_interval: Some(epoch_interval),
        }
        .start_async_ticker(&engine)
        .unwrap();
        Ok(Arc::new(Self {
            engine,
            _ticker: ticker,
            compilation: Arc::new(tokio::sync::Semaphore::new(compile_parallelism)),
            memory_limit,
            epoch_interval,
            sessions: Arc::new(tokio::sync::Semaphore::new(max_sessions)),
        }))
    }

    /// Cadence of the shared engine ticker.
    pub fn epoch_interval(&self) -> std::time::Duration {
        self.epoch_interval
    }

    /// Fail fast if the complete assembly cannot be admitted. Include lazy
    /// dependencies in `blocks`; dynamic spawning needs a separate reservation.
    pub fn reserve_session(self: &Arc<Self>, blocks: usize) -> Result<Arc<CoreWasmSession>> {
        let blocks = u32::try_from(blocks)
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| RuntimeError::wasm("admission", "invalid assembly block count"))?;
        let reservation = self
            .sessions
            .clone()
            .try_acquire_many_owned(blocks)
            .map_err(|_| RuntimeError::wasm("admission", "assembly capacity exhausted"))?;
        Ok(Arc::new(CoreWasmSession {
            engine: self.clone(),
            slots: Arc::new(tokio::sync::Semaphore::new(blocks as usize)),
            _reservation: reservation,
        }))
    }

    fn store_limits(&self) -> wasmtime::StoreLimits {
        wasmtime::StoreLimitsBuilder::new()
            .memory_size(self.memory_limit)
            .trap_on_grow_failure(true)
            .memories(1)
            .table_elements(100_000)
            .tables(1)
            .instances(1)
            .build()
    }

    /// Compile host-resolved bytes and inspect the manifest once using the same
    /// module. Callers verify artifact identity before preparation and cache the
    /// returned driver under that identity. Each execution gets a fresh store.
    pub async fn prepare(self: &Arc<Self>, bytes: Vec<u8>) -> Result<CoreWasmBlock> {
        let permit = self
            .compilation
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| RuntimeError::wasm("compile admission", e))?;
        let engine = self.engine.clone();
        let module = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            Module::new(&engine, bytes).map_err(|e| RuntimeError::wasm("module", e))
        })
        .await
        .map_err(|e| RuntimeError::wasm("compile task", e))??;
        let linker =
            CoreWasmBlock::async_linker::<NoOpStore, structfs_core_store::NoCodec>(&self.engine)?;
        let mut store = Store::new(
            &self.engine,
            CoreState {
                store: NoOpStore,
                codec: structfs_core_store::NoCodec,
                format: Format::OCTET_STREAM,
                limits: self.store_limits(),
                usage: None,
                memory: None,
                execution: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(10_000_000)
            .map_err(|e| RuntimeError::wasm("fuel", e))?;
        store
            .fuel_async_yield_interval(Some(100_000))
            .map_err(|e| RuntimeError::wasm("fuel yield", e))?;
        store.set_epoch_deadline(u64::MAX / 2);
        let instance = linker
            .instantiate_async(&mut store, &module)
            .await
            .map_err(|e| RuntimeError::wasm("instantiate", e))?;
        instance
            .get_typed_func::<(), i32>(&mut store, "run")
            .map_err(|e| RuntimeError::wasm("run; rebuild with the native binding", e))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "block_alloc")
            .map_err(|e| RuntimeError::wasm("block_alloc", e))?;
        let ret = alloc
            .call_async(&mut store, 8)
            .await
            .map_err(|e| RuntimeError::wasm("block_alloc", e))?;
        let manifest = instance
            .get_typed_func::<i32, i32>(&mut store, "manifest")
            .map_err(|e| RuntimeError::wasm("manifest; rebuild with the native binding", e))?;
        let code = manifest
            .call_async(&mut store, ret)
            .await
            .map_err(|e| RuntimeError::wasm("manifest", e))?;
        if code != status::OK {
            return Err(RuntimeError::Manifest(format!(
                "guest manifest returned status {code}"
            )));
        }
        let manifest = CoreWasmBlock::take_ret(&mut store, &instance, ret as u32 as usize)?;
        let _: serde_json::Value =
            serde_json::from_slice(&manifest).map_err(|e| RuntimeError::Manifest(e.to_string()))?;
        Ok(CoreWasmBlock {
            module_bytes: Vec::new(),
            prepared: Some((self.clone(), module, manifest)),
            session: None,
        })
    }
}

impl CoreWasmBlock {
    /// Synchronous loader preparation. Runtime loading is synchronous; compile
    /// and inspect once here, then share exactly that module with every run.
    pub(crate) fn prepare_for_loader(bytes: Vec<u8>) -> Result<Self> {
        let engine = CoreWasmEngine::new(1)?;
        let module =
            Module::new(&engine.engine, bytes).map_err(|e| RuntimeError::wasm("module", e))?;
        let linker = Self::linker::<NoOpStore, structfs_core_store::NoCodec>(&engine.engine)?;
        let mut store = Store::new(
            &engine.engine,
            CoreState {
                store: NoOpStore,
                codec: structfs_core_store::NoCodec,
                format: Format::OCTET_STREAM,
                limits: engine.store_limits(),
                usage: None,
                memory: None,
                execution: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(10_000_000)
            .map_err(|e| RuntimeError::wasm("fuel", e))?;
        store.set_epoch_deadline(u64::MAX / 2);
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| RuntimeError::wasm("instantiate", e))?;
        instance
            .get_typed_func::<(), i32>(&mut store, "run")
            .map_err(|e| RuntimeError::wasm("run", e))?;
        let ret = Self::alloc_ret(&mut store, &instance)?;
        let manifest = instance
            .get_typed_func::<i32, i32>(&mut store, "manifest")
            .map_err(|e| RuntimeError::wasm("manifest", e))?;
        let code = manifest
            .call(&mut store, ret as i32)
            .map_err(|e| RuntimeError::wasm("manifest", e))?;
        if code != status::OK {
            return Err(RuntimeError::Manifest(format!(
                "guest manifest returned status {code}"
            )));
        }
        let manifest = Self::take_ret(&mut store, &instance, ret)?;
        let _: serde_json::Value =
            serde_json::from_slice(&manifest).map_err(|e| RuntimeError::Manifest(e.to_string()))?;
        Ok(Self {
            module_bytes: Vec::new(),
            prepared: Some((engine, module, manifest)),
            session: None,
        })
    }

    /// Wrap core-module bytes (or wat text — wasmtime accepts both).
    #[cfg(test)]
    fn new(module_bytes: Vec<u8>) -> Self {
        Self {
            module_bytes,
            prepared: None,
            session: None,
        }
    }

    /// Bind shared code to an assembly's pre-reserved capacity. Guest memory
    /// remains fresh. The reservation must belong to the code's engine.
    pub fn in_session(&self, session: Arc<CoreWasmSession>) -> Result<Arc<Self>> {
        let (engine, module, manifest) = self
            .prepared
            .as_ref()
            .ok_or_else(|| RuntimeError::wasm("admission", "prepare the artifact first"))?;
        if !Arc::ptr_eq(engine, &session.engine) {
            return Err(RuntimeError::wasm(
                "admission",
                "reservation belongs to a different engine",
            ));
        }
        Ok(Arc::new(Self {
            module_bytes: Vec::new(),
            prepared: Some((engine.clone(), module.clone(), manifest.clone())),
            session: Some(session),
        }))
    }

    /// Load from a file.
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self> {
        Self::prepare_for_loader(std::fs::read(path)?)
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
                    check_host(&caller)?;
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
                    check_host(&caller)?;
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

    fn async_linker<S, C>(engine: &Engine) -> Result<Linker<CoreState<S, C>>>
    where
        S: structfs_core_store::AsyncReader + structfs_core_store::AsyncWriter + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let mut linker: Linker<CoreState<S, C>> = Linker::new(engine);

        linker
            .func_wrap_async(
                "structfs",
                "read",
                |mut caller: Caller<'_, CoreState<S, C>>,
                 (path_ptr, path_len, ret_ptr): (i32, i32, i32)| {
                    Box::new(async move {
                        sample_caller(&mut caller);
                        check_host(&caller)?;
                        let path_bytes = read_guest(&mut caller, path_ptr, path_len)?;
                        let path = match parse_path(&path_bytes) {
                            Ok(path) => path,
                            Err(message) => {
                                deliver_async(&mut caller, ret_ptr, message.as_bytes()).await?;
                                return Ok(status::INVALID_PATH);
                            }
                        };
                        let state = caller.data_mut();
                        let format = state.format.clone();
                        match state.store.read_async(&path).await {
                            Ok(None) => {
                                deliver_none(&mut caller, ret_ptr)?;
                                Ok(status::ABSENT)
                            }
                            Ok(Some(record)) => {
                                let state = caller.data_mut();
                                match record.into_bytes(&state.codec, &format) {
                                    Ok(bytes) => {
                                        let bytes = bytes.to_vec();
                                        deliver_async(&mut caller, ret_ptr, &bytes).await?;
                                        Ok(status::OK)
                                    }
                                    Err(e) => {
                                        let code = status_of(&e);
                                        deliver_async(
                                            &mut caller,
                                            ret_ptr,
                                            e.to_string().as_bytes(),
                                        )
                                        .await?;
                                        Ok(code)
                                    }
                                }
                            }
                            Err(e) => {
                                let code = status_of(&e);
                                deliver_async(&mut caller, ret_ptr, e.to_string().as_bytes())
                                    .await?;
                                Ok(code)
                            }
                        }
                    })
                },
            )
            .map_err(|e| RuntimeError::wasm("linker", e))?;

        linker
            .func_wrap_async(
                "structfs",
                "write",
                |mut caller: Caller<'_, CoreState<S, C>>,
                 (path_ptr, path_len, data_ptr, data_len, ret_ptr): (i32, i32, i32, i32, i32)| {
                    Box::new(async move {
                    check_host(&caller)?;
                    let path_bytes = read_guest(&mut caller, path_ptr, path_len)?;
                    let path = match parse_path(&path_bytes) {
                        Ok(path) => path,
                        Err(message) => {
                            deliver_async(&mut caller, ret_ptr, message.as_bytes()).await?;
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
                            deliver_async(&mut caller, ret_ptr, e.to_string().as_bytes()).await?;
                            return Ok(code);
                        }
                    };
                    let state = caller.data_mut();
                    match state.store.write_async(&path, Record::parsed(value)).await {
                        Ok(result_path) => {
                            let text = result_path.to_string();
                            deliver_async(&mut caller, ret_ptr, text.as_bytes()).await?;
                            Ok(status::OK)
                        }
                        Err(e) => {
                            let code = status_of(&e);
                            deliver_async(&mut caller, ret_ptr, e.to_string().as_bytes()).await?;
                            Ok(code)
                        }
                    }
                    })
                },
            )
            .map_err(|e| RuntimeError::wasm("linker", e))?;

        Ok(linker)
    }

    /// Run a core guest on Tokio. Imports suspend the Wasmtime fiber;
    /// parked guests do not own a blocking-pool thread.
    #[cfg(test)]
    async fn run_async<S, C>(
        &self,
        _id: BlockId,
        root: S,
        codec: C,
        format: Format,
        metering: &Metering,
        cancel: CancelToken,
    ) -> Result<i32>
    where
        S: structfs_core_store::AsyncReader + structfs_core_store::AsyncWriter + 'static,
        C: Codec + Send + Sync + 'static,
    {
        self.run_metered_async(
            _id,
            root,
            codec,
            format,
            metering,
            cancel,
            crate::ExecutionMeter::default(),
        )
        .await
    }

    /// Async execution with host-visible samples at imports, epoch yields and
    /// termination. Fuel counts Wasmtime units, not emulated instructions.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_metered_async<S, C>(
        &self,
        _id: BlockId,
        root: S,
        codec: C,
        format: Format,
        metering: &Metering,
        cancel: CancelToken,
        usage: crate::ExecutionMeter,
    ) -> Result<i32>
    where
        S: structfs_core_store::AsyncReader + structfs_core_store::AsyncWriter + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let prepared = match &self.prepared {
            Some(_) => Arc::new(self.clone()),
            None => {
                let engine = CoreWasmEngine::new(1)?;
                Arc::new(engine.prepare(self.module_bytes.clone()).await?)
            }
        };
        let outcome = prepared
            .run_host_async(
                root,
                codec,
                format,
                crate::ExecutionPolicy {
                    fuel: metering.fuel,
                    ..Default::default()
                },
                cancel,
            )
            .await;
        usage.replace(outcome.usage);
        outcome.result
    }

    #[allow(clippy::type_complexity)]
    fn instantiate<S, C>(
        &self,
        state: CoreState<S, C>,
        metering: &Metering,
        cancel: CancelToken,
    ) -> Result<(
        Store<CoreState<S, C>>,
        wasmtime::Instance,
        Option<EpochTicker>,
    )>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        if self.prepared.is_some() {
            return Err(RuntimeError::wasm(
                "execution",
                "prepared artifacts require run_async",
            ));
        }
        let mut config = wasmtime::Config::new();
        // Deterministic execution (spec 12, runtime obligation 2), on
        // unconditionally: the same guest bytes on the same answers must
        // compute the same result on every host, or replay's claim is
        // hollow. NaN canonicalization pins the one float behavior wasm
        // leaves loose; deterministic relaxed-SIMD pins the other.
        config.cranelift_nan_canonicalization(true);
        config.relaxed_simd_deterministic(true);
        metering.configure_engine(&mut config);
        let engine = Engine::new(&config).map_err(|e| RuntimeError::wasm("engine", e))?;
        let ticker = metering.start_ticker(&engine);
        let module = Module::new(&engine, &self.module_bytes)
            .map_err(|e| RuntimeError::wasm("module", e))?;
        let linker = Self::linker::<S, C>(&engine)?;
        let mut store = Store::new(&engine, state);
        store.limiter(|state| &mut state.limits);
        metering.arm_store(&mut store, cancel)?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(|e| RuntimeError::wasm("instantiate", e))?;
        Ok((store, instance, ticker))
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
        let range = checked_range(ptr, len, memory.data_size(&*store))
            .map_err(|e| RuntimeError::wasm("ret", e))?;
        Ok(memory.data(&*store)[range].to_vec())
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
    ///
    /// Fuel-bounded so a misbehaving manifest cannot hang the loader.
    pub fn manifest(&self) -> Result<Vec<u8>> {
        if let Some((_, _, manifest)) = &self.prepared {
            return Ok(manifest.clone());
        }
        use structfs_core_store::NoCodec;
        let state = CoreState {
            store: NoOpStore,
            codec: NoCodec,
            format: Format::OCTET_STREAM,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(64 * 1024 * 1024)
                .trap_on_grow_failure(true)
                .build(),
            usage: None,
            memory: None,
            execution: None,
        };
        let metering = Metering {
            fuel: Some(10_000_000),
            epoch_interval: None,
        };
        let (mut store, instance, _ticker) =
            self.instantiate(state, &metering, CancelToken::new())?;
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
    ///
    /// `cancel` interrupts *guest execution* via epoch interruption (when
    /// metering enables it); parked store reads are cancelled by the same
    /// token through the store contract.
    #[cfg(test)]
    fn run<S, C>(
        &self,
        _id: BlockId,
        root: S,
        codec: C,
        format: Format,
        metering: &Metering,
        cancel: CancelToken,
    ) -> Result<i32>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let state = CoreState {
            store: root,
            codec,
            format,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(64 * 1024 * 1024)
                .trap_on_grow_failure(true)
                .build(),
            usage: None,
            memory: None,
            execution: None,
        };
        let (mut store, instance, _ticker) = self.instantiate(state, metering, cancel)?;
        let run = instance
            .get_typed_func::<(), i32>(&mut store, "run")
            .map_err(|e| RuntimeError::wasm("run", e))?;
        // `{:#}` renders the whole cause chain: fuel exhaustion and
        // shutdown interrupts live below the trap's backtrace header.
        run.call(&mut store, ())
            .map_err(|e| RuntimeError::wasm("run", format!("{e:#}")))
    }
}

/// Whether wasm bytes are a component (layer 1) rather than a core
/// module — used to pick between this binding and the component one.
pub fn is_component(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[..4] == b"\0asm" && bytes[6] == 0x01
}

impl CoreWasmBlock {
    async fn host_admission(
        &self,
        policy: &crate::ExecutionPolicy,
        cancel: &CancelToken,
    ) -> Result<(Module, tokio::sync::OwnedSemaphorePermit, usize)> {
        policy.ensure_active(cancel)?;
        let (engine, module, _) = self.prepared.as_ref().ok_or_else(|| {
            RuntimeError::wasm(
                "execution",
                "prepare the artifact before starting owned execution",
            )
        })?;
        let memory = policy.memory_bytes.unwrap_or(engine.memory_limit);
        if memory == 0 || memory > engine.memory_limit {
            return Err(RuntimeError::wasm(
                "policy",
                "memory limit must be positive and cannot exceed engine ceiling",
            ));
        }
        let admission = async {
            if let Some(session) = &self.session {
                // Session reservations are already owned; retain the session and
                // use its local slots through an Arc semaphore.
                session.slots.clone().acquire_owned().await
            } else {
                engine.sessions.clone().acquire_owned().await
            }
        };
        let deadline = async {
            if let Some(deadline) = policy.deadline {
                tokio::time::sleep_until(deadline).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let permit = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(structfs_core_store::Error::cancelled("execution admission cancelled").into()),
            _ = deadline => return Err(structfs_core_store::Error::deadline_exceeded("execution admission deadline").into()),
            permit = admission => permit.map_err(|e| RuntimeError::wasm("admission", e))?,
        };
        policy.ensure_active(cancel)?;
        Ok((module.clone(), permit, memory))
    }

    #[allow(clippy::too_many_arguments)]
    fn host_store<S, C>(
        module: &Module,
        host: S,
        codec: C,
        format: Format,
        memory: usize,
        policy: &crate::ExecutionPolicy,
        cancel: &CancelToken,
        usage: &crate::ExecutionMeter,
    ) -> Store<CoreState<S, C>> {
        usage.configure_wasm(policy.fuel, Some(memory));
        Store::new(
            module.engine(),
            CoreState {
                store: host,
                codec,
                format,
                limits: wasmtime::StoreLimitsBuilder::new()
                    .memory_size(memory)
                    .trap_on_grow_failure(policy.growth_failure == crate::GrowthFailure::Trap)
                    .memories(1)
                    .instances(1)
                    .tables(1)
                    .table_elements(100_000)
                    .build(),
                usage: Some((usage.clone(), policy.fuel.unwrap_or(u64::MAX))),
                memory: None,
                execution: Some((policy.clone(), cancel.clone())),
            },
        )
    }

    fn arm_host<S: Send, C: Send>(
        store: &mut Store<CoreState<S, C>>,
        asynchronous: bool,
    ) -> Result<()> {
        store.limiter(|state| &mut state.limits);
        let (policy, cancel) = store.data().execution.as_ref().unwrap().clone();
        policy.ensure_active(&cancel)?;
        store
            .set_fuel(policy.fuel.unwrap_or(u64::MAX))
            .map_err(|e| RuntimeError::wasm("fuel", e))?;
        if asynchronous {
            store
                .fuel_async_yield_interval(Some(100_000))
                .map_err(|e| RuntimeError::wasm("fuel yield", e))?;
        }
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(move |_| {
            policy
                .ensure_active(&cancel)
                .map_err(|e| wasmtime::Error::msg(e.to_string()))?;
            Ok(if asynchronous {
                wasmtime::UpdateDeadline::Yield(1)
            } else {
                wasmtime::UpdateDeadline::Continue(1)
            })
        });
        Ok(())
    }

    fn finish_host<S, C>(
        mut store: Store<CoreState<S, C>>,
        mut result: Result<i32>,
        host_panicked: bool,
        usage: crate::ExecutionMeter,
    ) -> crate::ExecutionOutcome<S> {
        let (policy, cancel) = store.data().execution.as_ref().unwrap();
        if !host_panicked {
            if let Err(error) = policy.ensure_active(cancel) {
                result = Err(error);
            }
        }
        let fuel = policy
            .fuel
            .unwrap_or(u64::MAX)
            .saturating_sub(store.get_fuel().unwrap_or(0));
        if let Some(memory) = store.data().memory {
            usage.sample_wasm(fuel, memory.data_size(&store));
        } else {
            usage.sample_fuel(fuel);
        }
        // Extract the host before dropping the Wasm store, then mark memory reclaimed.
        let _ = &mut store;
        let host = store.into_data().store;
        usage.finish();
        crate::ExecutionOutcome {
            result,
            host,
            usage: usage.snapshot(),
            host_panicked,
        }
    }

    pub(crate) async fn run_host_sync<S, C>(
        self: &Arc<Self>,
        host: S,
        codec: C,
        format: Format,
        policy: crate::ExecutionPolicy,
        cancel: CancelToken,
    ) -> crate::ExecutionOutcome<S>
    where
        S: Reader + Writer + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let usage = crate::ExecutionMeter::default();
        let (module, permit, memory) = match self.host_admission(&policy, &cancel).await {
            Ok(admission) => admission,
            Err(error) => {
                usage.finish();
                return crate::ExecutionOutcome {
                    result: Err(error),
                    host,
                    usage: usage.snapshot(),
                    host_panicked: false,
                };
            }
        };
        // Retain code, engine ticker, and assembly reservation while blocking.
        let block = self.clone();
        tokio::task::spawn_blocking(move || {
            let _keep = (block, permit);
            let mut store = Self::host_store(
                &module, host, codec, format, memory, &policy, &cancel, &usage,
            );
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Self::arm_host(&mut store, false)?;
                let linker = Self::linker::<S, C>(module.engine())?;
                let instance = linker
                    .instantiate(&mut store, &module)
                    .map_err(|e| RuntimeError::wasm("instantiate", format!("{e:#}")))?;
                store.data_mut().memory = instance.get_memory(&mut store, "memory");
                let run = instance
                    .get_typed_func::<(), i32>(&mut store, "run")
                    .map_err(|e| RuntimeError::wasm("run", e))?;
                run.call(&mut store, ())
                    .map_err(|e| RuntimeError::wasm("run", format!("{e:#}")))
            }));
            let panicked = result.is_err();
            Self::finish_host(
                store,
                result.unwrap_or_else(|_| {
                    Err(RuntimeError::wasm(
                        "host panic",
                        "host state may be inconsistent",
                    ))
                }),
                panicked,
                usage,
            )
        })
        .await
        .expect("blocking execution catches host panics; keep executor alive until joined")
    }

    pub(crate) async fn run_host_async<S, C>(
        self: &Arc<Self>,
        host: S,
        codec: C,
        format: Format,
        policy: crate::ExecutionPolicy,
        cancel: CancelToken,
    ) -> crate::ExecutionOutcome<S>
    where
        S: structfs_core_store::AsyncReader + structfs_core_store::AsyncWriter + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let usage = crate::ExecutionMeter::default();
        let (module, _permit, memory) = match self.host_admission(&policy, &cancel).await {
            Ok(admission) => admission,
            Err(error) => {
                usage.finish();
                return crate::ExecutionOutcome {
                    result: Err(error),
                    host,
                    usage: usage.snapshot(),
                    host_panicked: false,
                };
            }
        };
        let mut store = Self::host_store(
            &module, host, codec, format, memory, &policy, &cancel, &usage,
        );
        let result = {
            let work = async {
                Self::arm_host(&mut store, true)?;
                let linker = Self::async_linker::<S, C>(module.engine())?;
                let instance = linker
                    .instantiate_async(&mut store, &module)
                    .await
                    .map_err(|e| RuntimeError::wasm("instantiate", format!("{e:#}")))?;
                store.data_mut().memory = instance.get_memory(&mut store, "memory");
                let run = instance
                    .get_typed_func::<(), i32>(&mut store, "run")
                    .map_err(|e| RuntimeError::wasm("run", e))?;
                run.call_async(&mut store, ())
                    .await
                    .map_err(|e| RuntimeError::wasm("run", format!("{e:#}")))
            };
            let mut work = std::pin::pin!(work);
            std::future::poll_fn(|cx| {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    std::future::Future::poll(work.as_mut(), cx)
                })) {
                    Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                    Ok(std::task::Poll::Ready(result)) => std::task::Poll::Ready(Ok(result)),
                    Err(_) => std::task::Poll::Ready(Err(())),
                }
            })
            .await
        };
        let panicked = result.is_err();
        Self::finish_host(
            store,
            result.unwrap_or_else(|_| {
                Err(RuntimeError::wasm(
                    "host panic",
                    "host state may be inconsistent",
                ))
            }),
            panicked,
            usage,
        )
    }
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
                .run(
                    BlockId::new(),
                    shared.clone(),
                    JsonCodec,
                    Format::JSON,
                    &Metering::disabled(),
                    CancelToken::new(),
                )
                .unwrap();
            assert_eq!(code, 0, "guest reported failure");
            shared
        };

        // The guest round-tripped the JSON bytes through its own memory.
        let output = store_after.read(&path!("output")).unwrap().unwrap();
        assert_eq!(output.as_value(), Some(&Value::from("hello from the host")));
    }

    #[test]
    fn every_transport_crosses_the_boundary() {
        use structfs_serde_store::MultiCodec;

        // The echo guest moves the payload bytes verbatim, so a
        // round trip proves host encode -> guest -> host decode for
        // each transport. The binary transports carry Value::Bytes
        // faithfully — the JSON tier cannot.
        let cases = [
            (Format::JSON, Value::from("hello over json")),
            (Format::CBOR, Value::Bytes(vec![0, 159, 146, 150])),
            (Format::FLEXBUFFERS, Value::Bytes(vec![255, 0, 7])),
            (
                Format::VALUE_JSON,
                Value::Array(vec![
                    Value::Unsigned(u64::MAX),
                    Value::Bytes(vec![0, 255]),
                    Value::Null,
                    Value::Float(-0.0),
                    Value::Float(f64::NAN),
                ]),
            ),
        ];
        for (format, value) in cases {
            let mut store = MemoryStore::new();
            store
                .write(&path!("input"), Record::parsed(value.clone()))
                .unwrap();

            let block = CoreWasmBlock::new(ECHO_GUEST.as_bytes().to_vec());
            let shared = structfs_core_store::Shared::new(store);
            let code = block
                .run(
                    BlockId::new(),
                    shared.clone(),
                    MultiCodec::standard(),
                    format.clone(),
                    &Metering::disabled(),
                    CancelToken::new(),
                )
                .unwrap();
            assert_eq!(code, 0, "guest reported failure under {format}");

            let mut after = shared.clone();
            let output = after.read(&path!("output")).unwrap().unwrap();
            assert!(
                output.as_value().unwrap().semantic_eq(&value),
                "mangled by {format}"
            );
        }
    }

    #[test]
    fn typed_errors_cross_as_status_codes() {
        let block = CoreWasmBlock::new(DENIED_GUEST.as_bytes().to_vec());
        let code = block
            .run(
                BlockId::new(),
                DenyStore,
                JsonCodec,
                Format::JSON,
                &Metering::disabled(),
                CancelToken::new(),
            )
            .unwrap();
        assert_eq!(code, 0, "guest did not observe status -2");
    }

    /// Spins forever: the metering test subject.
    const SPIN_GUEST: &str = r#"
        (module
          (memory (export "memory") 1)
          (func (export "block_alloc") (param i32) (result i32) (i32.const 4096))
          (data (i32.const 1088) "{\"serialization\":\"application/json\"}")
          (func (export "manifest") (param $ret i32) (result i32)
            (i32.store (local.get $ret) (i32.const 1088))
            (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 36))
            (i32.const 0))
          (func (export "run") (result i32)
            (loop $spin (br $spin))
            (i32.const 0)))
    "#;

    #[test]
    fn fuel_cap_stops_a_spinning_guest() {
        let block = CoreWasmBlock::new(SPIN_GUEST.as_bytes().to_vec());
        let metering = Metering {
            fuel: Some(1_000_000),
            epoch_interval: None,
        };
        let err = block
            .run(
                BlockId::new(),
                DenyStore,
                JsonCodec,
                Format::JSON,
                &metering,
                CancelToken::new(),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("fuel"),
            "expected fuel exhaustion, got: {err}"
        );
    }

    #[test]
    fn cancellation_interrupts_a_spinning_guest() {
        let block = std::sync::Arc::new(CoreWasmBlock::new(SPIN_GUEST.as_bytes().to_vec()));
        let cancel = CancelToken::new();
        let metering = Metering {
            fuel: None,
            epoch_interval: Some(std::time::Duration::from_millis(2)),
        };

        let runner = {
            let block = block.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                block.run(
                    BlockId::new(),
                    DenyStore,
                    JsonCodec,
                    Format::JSON,
                    &metering,
                    cancel,
                )
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.cancel();

        let err = runner.join().unwrap().unwrap_err();
        assert!(
            err.to_string().contains("interrupted"),
            "expected shutdown interrupt, got: {err}"
        );
    }

    #[test]
    fn no_op_store_read_returns_none() {
        let mut store = NoOpStore;
        assert!(store.read(&path!("some/path")).unwrap().is_none());
    }

    #[test]
    fn no_op_store_write_echoes_path() {
        let mut store = NoOpStore;
        let record = Record::raw(bytes::Bytes::from_static(b"data"), Format::OCTET_STREAM);
        assert_eq!(
            store.write(&path!("some/path"), record).unwrap(),
            path!("some/path")
        );
    }

    #[test]
    fn transfer_ranges_are_bounded_before_copying() {
        assert!(checked_range(usize::MAX, 1, usize::MAX).is_err());
        assert!(checked_range(0, MAX_TRANSFER_BYTES + 1, usize::MAX).is_err());
        assert!(checked_range(65535, 2, 65536).is_err());
        assert_eq!(checked_range(65536, 0, 65536).unwrap(), 65536..65536);
    }

    #[tokio::test]
    async fn malformed_import_ranges_trap_on_both_execution_paths() {
        for (ptr, len) in [(0, -1), (-1, 1), (65535, 2)] {
            let guest = format!(
                r#"(module
                (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
                (memory (export "memory") 1)
                (data (i32.const 512) "{{}}")
                (func (export "block_alloc") (param i32) (result i32) i32.const 1024)
                (func (export "manifest") (param $ret i32) (result i32)
                    local.get $ret i32.const 512 i32.store
                    local.get $ret i32.const 4 i32.add i32.const 2 i32.store i32.const 0)
                (func (export "run") (result i32)
                    (call $read (i32.const {ptr}) (i32.const {len}) (i32.const 0))))"#
            );
            let block = CoreWasmBlock::new(guest.into_bytes());
            let err = block
                .run(
                    BlockId::new(),
                    NoOpStore,
                    JsonCodec,
                    Format::JSON,
                    &Metering::disabled(),
                    CancelToken::new(),
                )
                .unwrap_err();
            assert!(
                err.to_string().contains("guest transfer")
                    || err.to_string().contains("out of bounds"),
                "{err}"
            );
            let err = block
                .run_async(
                    BlockId::new(),
                    structfs_core_store::SyncToAsync::new(NoOpStore),
                    JsonCodec,
                    Format::JSON,
                    &Metering::disabled(),
                    CancelToken::new(),
                )
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("guest transfer")
                    || err.to_string().contains("out of bounds"),
                "{err}"
            );
        }
    }

    #[test]
    fn malformed_manifest_range_is_rejected_before_allocation() {
        let guest = ECHO_GUEST.replace("(i32.const 54)", "(i32.const -1)");
        let err = CoreWasmBlock::new(guest.into_bytes())
            .manifest()
            .unwrap_err();
        assert!(err.to_string().contains("guest transfer"), "{err}");
    }

    #[test]
    fn async_echo_runs_with_one_blocking_worker() {
        // A guest occupying the sole blocking worker would deadlock when
        // SyncToAsync dispatches the guest's provider operation to that pool.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut store = structfs_core_store::Shared::new(MemoryStore::new());
            let value = Value::from("fresh async guest");
            store
                .write(&path!("input"), Record::parsed(value.clone()))
                .unwrap();
            let block = CoreWasmBlock::new(ECHO_GUEST.as_bytes().to_vec());
            let code = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                block.run_async(
                    BlockId::new(),
                    structfs_core_store::SyncToAsync::new(store.clone()),
                    JsonCodec,
                    Format::JSON,
                    &Metering::default(),
                    CancelToken::new(),
                ),
            )
            .await
            .expect("guest held the blocking worker")
            .unwrap();
            assert_eq!(code, 0);
            assert_eq!(
                store.read(&path!("output")).unwrap().unwrap().as_value(),
                Some(&value)
            );
        });
    }

    #[tokio::test]
    async fn async_spin_yields_to_cancellation_on_current_thread() {
        let block = CoreWasmBlock::new(SPIN_GUEST.as_bytes().to_vec());
        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            trigger.cancel();
        });
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            block.run_async(
                BlockId::new(),
                structfs_core_store::SyncToAsync::new(NoOpStore),
                JsonCodec,
                Format::JSON,
                &Metering::default(),
                cancel,
            ),
        )
        .await
        .expect("guest starved the executor")
        .unwrap_err();
        timer.await.unwrap();
        assert!(
            err.to_string().contains("interrupted") || err.to_string().contains("cancelled"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn prepared_memory_limits_cover_instantiation_and_growth() {
        let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
        let too_large = SPIN_GUEST.replace(
            "(memory (export \"memory\") 1)",
            "(memory (export \"memory\") 2)",
        );
        assert!(engine.prepare(too_large.into_bytes()).await.is_err());
        let guest = SPIN_GUEST.replace(
            "(loop $spin (br $spin))",
            r#"
            (if (i32.ne (memory.grow (i32.const 1)) (i32.const -1))
                (then unreachable))"#,
        );
        let prepared = engine.prepare(guest.into_bytes()).await.unwrap();
        assert!(prepared
            .run_async(
                BlockId::new(),
                NoOpStore,
                JsonCodec,
                Format::JSON,
                &Metering::disabled(),
                CancelToken::new()
            )
            .await
            .is_err());
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
