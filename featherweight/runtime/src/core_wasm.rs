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
//! stateless; the typed error taxonomy crosses as negative status codes
//! ([`crate::protocol::ErrorKind::status`]).
//!
//! There is one engine model: a [`CoreWasmEngine`] compiles and inspects
//! an artifact once ([`CoreWasmEngine::prepare`]), and every run of the
//! resulting [`CoreWasmBlock`] gets a fresh Wasmtime store. The import
//! bodies are written once and instantiated for the synchronous and the
//! asynchronous linker.

use std::sync::Arc;

use structfs_core_store::{
    AsyncReader, AsyncWriter, Codec, Error as StoreError, Format, NoCodec, Path, Reader, Record,
    Writer,
};
use structfs_handles::CancelToken;
use wasmtime::{Caller, Engine, Extern, Linker, Module, Store, TypedFunc};

use crate::adapter;
use crate::error::{Result, RuntimeError};
use crate::protocol::{status, ErrorKind};
use crate::{ExecutionMeter, ExecutionOutcome, ExecutionPolicy, GrowthFailure};

/// UTF-8 diagnostics remain readable to old guests. Updated SDKs recognize the
/// versioned JSON envelope only for codec errors; other errors remain plain text.
fn host_diagnostic(error: &StoreError) -> String {
    match error.codec_diagnostic() {
        Some(codec) => {
            serde_json::json!({"structfs_error": 1, "message": error.to_string(), "codec": codec})
                .to_string()
        }
        None => error.to_string(),
    }
}

/// Host transfer budget, independent of the guest's linear-memory size.
const MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

/// Per-instance host state: the block's namespace plus its codec.
struct CoreState<S, C> {
    store: S,
    codec: C,
    format: Format,
    limits: wasmtime::StoreLimits,
    /// The run's meter and its initial fuel, for samples at imports.
    meter: Option<(ExecutionMeter, u64)>,
    memory: Option<wasmtime::Memory>,
    /// Checked at every import: a cancelled or expired run dispatches
    /// nothing further.
    execution: Option<(ExecutionPolicy, CancelToken)>,
}

/// What an import answers: a status and the payload for the ret record
/// (`None` zeroes it).
struct Reply {
    status: i32,
    payload: Option<Vec<u8>>,
}

impl Reply {
    fn ok(payload: Vec<u8>) -> Self {
        Self {
            status: status::OK,
            payload: Some(payload),
        }
    }

    fn error(error: &StoreError) -> Self {
        Self {
            status: ErrorKind::of(error).status(),
            payload: Some(host_diagnostic(error).into_bytes()),
        }
    }

    fn invalid_path(message: String) -> Self {
        Self {
            status: status::INVALID_PATH,
            payload: Some(message.into_bytes()),
        }
    }

    fn read<C: Codec>(
        codec: &C,
        format: &Format,
        result: std::result::Result<Option<Record>, StoreError>,
    ) -> Self {
        match result.and_then(|found| {
            found
                .map(|record| record.into_bytes(codec, format))
                .transpose()
        }) {
            Ok(Some(bytes)) => Reply::ok(bytes.to_vec()),
            Ok(None) => Reply {
                status: status::ABSENT,
                payload: None,
            },
            Err(error) => Reply::error(&error),
        }
    }

    fn written(result: std::result::Result<Path, StoreError>) -> Self {
        match result {
            Ok(path) => Reply::ok(path.to_string().into_bytes()),
            Err(error) => Reply::error(&error),
        }
    }
}

fn check_host<S, C>(caller: &Caller<'_, CoreState<S, C>>) -> wasmtime::Result<()> {
    if let Some((policy, cancel)) = &caller.data().execution {
        policy
            .ensure_active(cancel)
            .map_err(|e| wasmtime::Error::msg(e.to_string()))?;
    }
    Ok(())
}

fn sample_caller<S, C>(caller: &mut Caller<'_, CoreState<S, C>>) {
    if let Some((meter, initial)) = caller.data().meter.clone() {
        if let Some(memory) = caller.get_export("memory").and_then(|e| e.into_memory()) {
            meter.sample_wasm(
                initial.saturating_sub(caller.get_fuel().unwrap_or(initial)),
                memory.data_size(&*caller),
            );
        }
    }
}

fn checked_range(
    ptr: usize,
    len: usize,
    memory_len: usize,
) -> wasmtime::Result<std::ops::Range<usize>> {
    if len > MAX_TRANSFER_BYTES {
        return Err(wasmtime::Error::msg("guest transfer exceeds 64 MiB limit"));
    }
    let end = ptr
        .checked_add(len)
        .filter(|end| *end <= memory_len)
        .ok_or_else(|| wasmtime::Error::msg("guest memory range out of bounds"))?;
    Ok(ptr..end)
}

fn guest_memory<S, C>(
    caller: &mut Caller<'_, CoreState<S, C>>,
) -> wasmtime::Result<wasmtime::Memory> {
    caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or_else(|| wasmtime::Error::msg("guest exports no memory"))
}

fn guest_alloc<S, C>(
    caller: &mut Caller<'_, CoreState<S, C>>,
) -> wasmtime::Result<TypedFunc<i32, i32>> {
    caller
        .get_export("block_alloc")
        .and_then(Extern::into_func)
        .ok_or_else(|| wasmtime::Error::msg("guest exports no block_alloc"))?
        .typed(&mut *caller)
}

/// Read guest memory into a Vec.
fn read_guest<S, C>(
    caller: &mut Caller<'_, CoreState<S, C>>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<Vec<u8>> {
    let memory = guest_memory(caller)?;
    let range = checked_range(
        ptr as u32 as usize,
        len as u32 as usize,
        memory.data_size(&*caller),
    )?;
    Ok(memory.data(&*caller)[range].to_vec())
}

/// The path argument of an import: a guest-contract violation traps; an
/// invalid path is an answer (status -7).
fn guest_path<S, C>(
    caller: &mut Caller<'_, CoreState<S, C>>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<std::result::Result<Path, String>> {
    let bytes = read_guest(caller, ptr, len)?;
    Ok(std::str::from_utf8(&bytes)
        .map_err(|e| format!("path is not UTF-8: {e}"))
        .and_then(|text| Path::parse(text).map_err(|e| e.to_string())))
}

fn ret_record(ptr: u32, len: u32) -> [u8; 8] {
    let mut record = [0u8; 8];
    record[..4].copy_from_slice(&ptr.to_le_bytes());
    record[4..].copy_from_slice(&len.to_le_bytes());
    record
}

/// Deliver a reply: allocate via the guest's `block_alloc` (`$call` is
/// `call` or `call_async`, the latter with `await`), copy the payload, and
/// store `{ptr, len}` at the ret pointer. Violations of the guest contract
/// trap. Evaluates to the reply's status.
macro_rules! deliver {
    ($caller:ident, $ret_ptr:expr, $reply:expr, $call:ident $(, $aw:ident)?) => {{
        let reply: Reply = $reply;
        let memory = guest_memory(&mut $caller)?;
        let ret_ptr = $ret_ptr as u32 as usize;
        checked_range(ret_ptr, 8, memory.data_size(&$caller))?;
        let payload = reply.payload.unwrap_or_default();
        if payload.len() > MAX_TRANSFER_BYTES {
            return Err(wasmtime::Error::msg("guest transfer exceeds 64 MiB limit"));
        }
        let ptr = if payload.is_empty() {
            0u32
        } else {
            let alloc = guest_alloc(&mut $caller)?;
            let ptr = alloc.$call(&mut $caller, payload.len() as i32)$(.$aw)??;
            memory.write(&mut $caller, ptr as u32 as usize, &payload)?;
            ptr as u32
        };
        memory.write(&mut $caller, ret_ptr, &ret_record(ptr, payload.len() as u32))?;
        Ok(reply.status)
    }};
}

/// The body of the `structfs.read` import, shared by both linkers.
macro_rules! read_import {
    ($caller:ident, $path_ptr:ident, $path_len:ident, $ret_ptr:ident,
     $read:ident, $call:ident $(, $aw:ident)?) => {{
        sample_caller(&mut $caller);
        check_host(&$caller)?;
        let reply = match guest_path(&mut $caller, $path_ptr, $path_len)? {
            Err(message) => Reply::invalid_path(message),
            Ok(path) => {
                let result = $caller.data_mut().store.$read(&path)$(.$aw)?;
                let state = $caller.data();
                Reply::read(&state.codec, &state.format, result)
            }
        };
        deliver!($caller, $ret_ptr, reply, $call $(, $aw)?)
    }};
}

/// The body of the `structfs.write` import, shared by both linkers.
macro_rules! write_import {
    ($caller:ident, $path_ptr:ident, $path_len:ident, $data_ptr:ident, $data_len:ident,
     $ret_ptr:ident, $write:ident, $call:ident $(, $aw:ident)?) => {{
        sample_caller(&mut $caller);
        check_host(&$caller)?;
        let reply = match guest_path(&mut $caller, $path_ptr, $path_len)? {
            Err(message) => Reply::invalid_path(message),
            Ok(path) => {
                let data = read_guest(&mut $caller, $data_ptr, $data_len)?;
                let state = $caller.data_mut();
                // Stores receive Values; the boundary carries bytes.
                match state.codec.decode(&bytes::Bytes::from(data), &state.format) {
                    Err(error) => Reply::error(&error),
                    Ok(value) => Reply::written(
                        state.store.$write(&path, Record::parsed(value))$(.$aw)?,
                    ),
                }
            }
        };
        deliver!($caller, $ret_ptr, reply, $call $(, $aw)?)
    }};
}

fn sync_linker<S, C>(engine: &Engine) -> Result<Linker<CoreState<S, C>>>
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
             -> wasmtime::Result<i32> {
                read_import!(caller, path_ptr, path_len, ret_ptr, read, call)
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
             -> wasmtime::Result<i32> {
                write_import!(caller, path_ptr, path_len, data_ptr, data_len, ret_ptr, write, call)
            },
        )
        .map_err(|e| RuntimeError::wasm("linker", e))?;
    Ok(linker)
}

fn async_linker<S, C>(engine: &Engine) -> Result<Linker<CoreState<S, C>>>
where
    S: AsyncReader + AsyncWriter + Send + 'static,
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
                    read_import!(caller, path_ptr, path_len, ret_ptr, read_async, call_async, await)
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
                    write_import!(
                        caller, path_ptr, path_len, data_ptr, data_len, ret_ptr, write_async,
                        call_async, await
                    )
                })
            },
        )
        .map_err(|e| RuntimeError::wasm("linker", e))?;
    Ok(linker)
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
impl AsyncReader for NoOpStore {
    async fn read_async(&mut self, path: &Path) -> std::result::Result<Option<Record>, StoreError> {
        self.read(path)
    }
}
#[async_trait::async_trait]
impl AsyncWriter for NoOpStore {
    async fn write_async(
        &mut self,
        path: &Path,
        record: Record,
    ) -> std::result::Result<Path, StoreError> {
        self.write(path, record)
    }
}

/// Shared compilation service for fresh guest stores. Create inside Tokio and
/// keep its executor alive until all prepared artifacts and runs finish: the
/// engine's epoch ticker, its compilations, and synchronous runs all use that
/// runtime. Artifact retention/eviction belongs to the embedding host.
pub struct CoreWasmEngine {
    engine: Engine,
    handle: tokio::runtime::Handle,
    _ticker: adapter::EpochTicker,
    compilation: Arc<tokio::sync::Semaphore>,
    memory_limit: usize,
    sessions: Arc<tokio::sync::Semaphore>,
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
    ///
    /// At most 10,000 runs hold a store at once (64 MiB linear-memory
    /// ceiling each); a run that finds every slot taken waits for one,
    /// cancellably and within its deadline — observe the pressure with
    /// [`CoreWasmEngine::available_sessions`]. Use
    /// [`CoreWasmEngine::with_limits`] for other caps; a runtime's built-in
    /// `.wasm` loader uses these defaults unless given an engine with
    /// `RuntimeConfig::with_core_engine`.
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
            adapter::DEFAULT_EPOCH_INTERVAL,
        )
    }

    /// Engine-wide epoch cadence: how often a running guest observes
    /// cancellation and its deadline. Any positive interval is accepted.
    /// Per-run policy never changes the shared ticker.
    pub fn with_epoch_interval(
        compile_parallelism: usize,
        max_sessions: usize,
        memory_limit: usize,
        epoch_interval: std::time::Duration,
    ) -> Result<Arc<Self>> {
        if max_sessions == 0 || memory_limit == 0 {
            return Err(RuntimeError::EngineConfig(
                "session and memory limits must be positive".into(),
            ));
        }
        if compile_parallelism == 0 {
            return Err(RuntimeError::EngineConfig(
                "compile parallelism must be positive".into(),
            ));
        }
        let mut config = wasmtime::Config::new();
        adapter::configure_engine(&mut config);
        let engine = Engine::new(&config).map_err(|e| RuntimeError::EngineConfig(e.to_string()))?;
        let ticker = adapter::start_ticker(&engine, epoch_interval)?;
        Ok(Arc::new(Self {
            engine,
            handle: tokio::runtime::Handle::current(),
            _ticker: ticker,
            compilation: Arc::new(tokio::sync::Semaphore::new(compile_parallelism)),
            memory_limit,
            sessions: Arc::new(tokio::sync::Semaphore::new(max_sessions)),
        }))
    }

    /// Execution slots currently free. A run that starts when this is zero
    /// waits (cancellably, within its deadline) until another run ends.
    pub fn available_sessions(&self) -> usize {
        self.sessions.available_permits()
    }

    /// Fail fast if the complete assembly cannot be admitted. Include lazy
    /// dependencies in `blocks`; dynamic spawning needs a separate reservation.
    pub fn reserve_session(self: &Arc<Self>, blocks: usize) -> Result<Arc<CoreWasmSession>> {
        let blocks = u32::try_from(blocks)
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| RuntimeError::Admission("invalid assembly block count".into()))?;
        let reservation = self
            .sessions
            .clone()
            .try_acquire_many_owned(blocks)
            .map_err(|_| RuntimeError::Admission("assembly capacity exhausted".into()))?;
        Ok(Arc::new(CoreWasmSession {
            engine: self.clone(),
            slots: Arc::new(tokio::sync::Semaphore::new(blocks as usize)),
            _reservation: reservation,
        }))
    }

    fn store_limits(&self, memory: usize, growth: GrowthFailure) -> wasmtime::StoreLimits {
        wasmtime::StoreLimitsBuilder::new()
            .memory_size(memory)
            .trap_on_grow_failure(growth == GrowthFailure::Trap)
            .memories(1)
            .table_elements(100_000)
            .tables(1)
            .instances(1)
            .build()
    }

    /// Compile host-resolved bytes and inspect the manifest once using the same
    /// module. Callers verify artifact identity before preparation and cache the
    /// returned block under that identity. Each execution gets a fresh store.
    pub async fn prepare(self: &Arc<Self>, bytes: Vec<u8>) -> Result<CoreWasmBlock> {
        let permit = self
            .compilation
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| RuntimeError::Admission(e.to_string()))?;
        let engine = self.clone();
        self.handle
            .spawn_blocking(move || {
                let _permit = permit;
                engine.prepare_blocking(&bytes)
            })
            .await
            .map_err(|e| RuntimeError::task_failed("compile task", e))?
    }

    /// Compile and inspect on the calling thread. The runtime's built-in
    /// loader uses this; embedders use [`CoreWasmEngine::prepare`].
    pub(crate) fn prepare_blocking(self: &Arc<Self>, bytes: &[u8]) -> Result<CoreWasmBlock> {
        let module =
            Module::new(&self.engine, bytes).map_err(|e| RuntimeError::wasm("module", e))?;
        let manifest = self.inspect(&module)?;
        Ok(CoreWasmBlock {
            engine: self.clone(),
            module,
            manifest,
            session: None,
        })
    }

    /// Instantiate once over a no-op store and call the guest's `manifest`
    /// export (spec 01), fuel-bounded. Also checks the `run` export exists.
    fn inspect(&self, module: &Module) -> Result<Vec<u8>> {
        let linker = sync_linker::<NoOpStore, NoCodec>(&self.engine)?;
        let mut store = Store::new(
            &self.engine,
            CoreState {
                store: NoOpStore,
                codec: NoCodec,
                format: Format::OCTET_STREAM,
                limits: self.store_limits(self.memory_limit, GrowthFailure::Trap),
                meter: None,
                memory: None,
                execution: None,
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(adapter::MANIFEST_FUEL)
            .map_err(|e| RuntimeError::wasm("fuel", e))?;
        store.set_epoch_deadline(u64::MAX / 2);
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| RuntimeError::wasm("instantiate", e))?;
        instance
            .get_typed_func::<(), i32>(&mut store, "run")
            .map_err(|e| RuntimeError::wasm("run export", e))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "block_alloc")
            .map_err(|e| RuntimeError::wasm("block_alloc", e))?;
        let ret = alloc
            .call(&mut store, 8)
            .map_err(|e| RuntimeError::wasm("block_alloc", e))?;
        let code = instance
            .get_typed_func::<i32, i32>(&mut store, "manifest")
            .map_err(|e| RuntimeError::wasm("manifest export", e))?
            .call(&mut store, ret)
            .map_err(|e| RuntimeError::wasm("manifest", e))?;
        if code != status::OK {
            return Err(RuntimeError::Manifest(format!(
                "guest manifest returned status {code}"
            )));
        }
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| RuntimeError::wasm("memory", "guest exports no memory"))?;
        let mut record = [0u8; 8];
        memory
            .read(&store, ret as u32 as usize, &mut record)
            .map_err(|e| RuntimeError::wasm("ret", e))?;
        let [p0, p1, p2, p3, l0, l1, l2, l3] = record;
        let range = checked_range(
            u32::from_le_bytes([p0, p1, p2, p3]) as usize,
            u32::from_le_bytes([l0, l1, l2, l3]) as usize,
            memory.data_size(&store),
        )
        .map_err(|e| RuntimeError::wasm("ret", e))?;
        let manifest = memory.data(&store)[range].to_vec();
        serde_json::from_slice::<serde_json::Value>(&manifest)
            .map_err(|e| RuntimeError::Manifest(e.to_string()))?;
        Ok(manifest)
    }
}

/// Whether wasm bytes are a component (layer 1) rather than a core
/// module — used to pick between this binding and the component one.
pub fn is_component(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && &bytes[..4] == b"\0asm" && bytes[6] == 0x01
}

/// A prepared block in the core-wasm binding: compiled code plus its
/// manifest, bound to the engine that compiled it. Start runs with
/// [`CoreWasmBlock::start_sync`] / [`CoreWasmBlock::start_async`], or
/// register it with [`crate::RuntimeConfig::register_core_artifact`].
#[derive(Clone)]
pub struct CoreWasmBlock {
    engine: Arc<CoreWasmEngine>,
    module: Module,
    manifest: Vec<u8>,
    session: Option<Arc<CoreWasmSession>>,
}

impl std::fmt::Debug for CoreWasmBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreWasmBlock")
            .field("manifest", &String::from_utf8_lossy(&self.manifest))
            .field("session", &self.session.is_some())
            .finish_non_exhaustive()
    }
}

/// Everything one run needs besides the prepared code.
pub(crate) struct HostRun<S, C> {
    pub(crate) host: S,
    pub(crate) codec: C,
    pub(crate) format: Format,
    pub(crate) policy: ExecutionPolicy,
    pub(crate) cancel: CancelToken,
    /// Samples land here during the run, so an instance meter shows live
    /// usage; it is finished when the run ends.
    pub(crate) meter: ExecutionMeter,
}

impl CoreWasmBlock {
    /// Bind shared code to an assembly's pre-reserved capacity. Guest memory
    /// remains fresh. The reservation must belong to the code's engine.
    pub fn in_session(&self, session: Arc<CoreWasmSession>) -> Result<Arc<Self>> {
        if !Arc::ptr_eq(&self.engine, &session.engine) {
            return Err(RuntimeError::Admission(
                "reservation belongs to a different engine".into(),
            ));
        }
        Ok(Arc::new(Self {
            session: Some(session),
            ..self.clone()
        }))
    }

    /// The block's JSON manifest (spec 01), captured at preparation.
    pub fn manifest(&self) -> &[u8] {
        &self.manifest
    }

    /// Wait for an execution slot (the session's, or the engine's) and
    /// validate the policy. Returns the slot and the memory ceiling.
    async fn admit(
        &self,
        policy: &ExecutionPolicy,
        cancel: &CancelToken,
    ) -> Result<(tokio::sync::OwnedSemaphorePermit, usize)> {
        policy.ensure_active(cancel)?;
        let memory = policy.memory_bytes.unwrap_or(self.engine.memory_limit);
        if memory == 0 || memory > self.engine.memory_limit {
            return Err(RuntimeError::Policy(
                "memory limit must be positive and cannot exceed engine ceiling".into(),
            ));
        }
        let slots = match &self.session {
            // Session reservations are already owned; use the session's
            // local slots.
            Some(session) => session.slots.clone(),
            None => self.engine.sessions.clone(),
        };
        let deadline = async {
            match policy.deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };
        let permit = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(StoreError::cancelled("execution admission cancelled").into()),
            _ = deadline => return Err(StoreError::deadline_exceeded("execution admission deadline").into()),
            permit = slots.acquire_owned() => permit.map_err(|e| RuntimeError::Admission(e.to_string()))?,
        };
        policy.ensure_active(cancel)?;
        Ok((permit, memory))
    }

    fn new_store<S, C>(
        &self,
        memory: usize,
        run: HostRun<S, C>,
    ) -> (Store<CoreState<S, C>>, ExecutionMeter) {
        let HostRun {
            host,
            codec,
            format,
            policy,
            cancel,
            meter,
        } = run;
        meter.configure_wasm(policy.fuel, Some(memory));
        let limits = self.engine.store_limits(memory, policy.growth_failure);
        let state = CoreState {
            store: host,
            codec,
            format,
            limits,
            meter: Some((meter.clone(), policy.fuel.unwrap_or(u64::MAX))),
            memory: None,
            execution: Some((policy, cancel)),
        };
        (Store::new(&self.engine.engine, state), meter)
    }

    fn arm<S: 'static, C: 'static>(
        store: &mut Store<CoreState<S, C>>,
        asynchronous: bool,
    ) -> Result<()> {
        store.limiter(|state| &mut state.limits);
        let (policy, cancel) = store
            .data()
            .execution
            .clone()
            .ok_or_else(|| RuntimeError::Policy("store has no execution policy".into()))?;
        policy.ensure_active(&cancel)?;
        let fuel = policy.fuel;
        adapter::arm_store(store, fuel, asynchronous, move || {
            policy.ensure_active(&cancel).map_err(|e| e.to_string())
        })
    }

    /// Record final usage and hand the host back. The host is extracted
    /// before the Wasm store drops; memory then reads as reclaimed.
    fn finish<S, C>(
        mut store: Store<CoreState<S, C>>,
        mut result: Result<i32>,
        host_panicked: bool,
        meter: ExecutionMeter,
    ) -> ExecutionOutcome<S> {
        let accounted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Some((policy, cancel)) = &store.data().execution {
                if !host_panicked {
                    if let Err(error) = policy.ensure_active(cancel) {
                        result = Err(error);
                    }
                }
            }
            let initial = store
                .data()
                .meter
                .as_ref()
                .map_or(u64::MAX, |(_, fuel)| *fuel);
            let fuel = initial.saturating_sub(store.get_fuel().unwrap_or(0));
            match store.data().memory {
                Some(memory) => meter.sample_wasm(fuel, memory.data_size(&store)),
                None => meter.sample_fuel(fuel),
            }
        }));
        let _ = &mut store;
        let host = store.into_data().store;
        meter.finish();
        ExecutionOutcome {
            result,
            host,
            usage: meter.snapshot(),
            host_panicked: host_panicked || accounted.is_err(),
        }
    }

    fn refused<S>(host: S, error: RuntimeError, meter: ExecutionMeter) -> ExecutionOutcome<S> {
        meter.finish();
        ExecutionOutcome {
            result: Err(error),
            host,
            usage: meter.snapshot(),
            host_panicked: false,
        }
    }

    fn host_panic<T>(_: T) -> RuntimeError {
        RuntimeError::HostPanic("host state may be inconsistent".into())
    }

    /// Run on the engine's blocking pool with synchronous host effects.
    ///
    /// Returns `None` only when the host state itself was destroyed (a
    /// panic in the engine outside every guarded region); a run that never
    /// started, or whose host panicked, still returns the host.
    pub(crate) async fn run_host_sync<S, C>(
        self: &Arc<Self>,
        run: HostRun<S, C>,
    ) -> Option<ExecutionOutcome<S>>
    where
        S: Reader + Writer + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let (permit, memory) = match self.admit(&run.policy, &run.cancel).await {
            Ok(admitted) => admitted,
            Err(error) => return Some(Self::refused(run.host, error, run.meter)),
        };
        // The run parks in a slot until the worker takes it, so a worker
        // that never starts (its executor shut down) cannot lose the host.
        let slot = Arc::new(std::sync::Mutex::new(Some(run)));
        let pending = slot.clone();
        // Retain code, engine ticker, and assembly reservation while blocking.
        let block = self.clone();
        let joined = self
            .engine
            .handle
            .spawn_blocking(move || {
                let _permit = permit;
                let run = pending.lock().unwrap_or_else(|e| e.into_inner()).take()?;
                let (mut store, meter) = block.new_store(memory, run);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Self::arm(&mut store, false)?;
                    let linker = sync_linker::<S, C>(&block.engine.engine)?;
                    let instance = linker
                        .instantiate(&mut store, &block.module)
                        .map_err(|e| RuntimeError::wasm("instantiate", format!("{e:#}")))?;
                    store.data_mut().memory = instance.get_memory(&mut store, "memory");
                    instance
                        .get_typed_func::<(), i32>(&mut store, "run")
                        .map_err(|e| RuntimeError::wasm("run", e))?
                        .call(&mut store, ())
                        .map_err(|e| RuntimeError::wasm("run", format!("{e:#}")))
                }));
                let panicked = result.is_err();
                Some(Self::finish(
                    store,
                    result.unwrap_or_else(|e| Err(Self::host_panic(e))),
                    panicked,
                    meter,
                ))
            })
            .await;
        match joined {
            Ok(outcome) => outcome,
            Err(join) => {
                let run = slot.lock().unwrap_or_else(|e| e.into_inner()).take()?;
                let error = if join.is_panic() {
                    Self::host_panic(join)
                } else {
                    RuntimeError::ExecutionLost("the blocking worker never started".into())
                };
                let mut outcome = Self::refused(run.host, error, run.meter);
                outcome.host_panicked = matches!(outcome.result, Err(RuntimeError::HostPanic(_)));
                Some(outcome)
            }
        }
    }

    /// Run on the calling task with asynchronous host effects: imports
    /// suspend the Wasmtime fiber, so a parked guest owns no thread.
    pub(crate) async fn run_host_async<S, C>(
        self: &Arc<Self>,
        run: HostRun<S, C>,
    ) -> ExecutionOutcome<S>
    where
        S: AsyncReader + AsyncWriter + Send + 'static,
        C: Codec + Send + Sync + 'static,
    {
        let (_permit, memory) = match self.admit(&run.policy, &run.cancel).await {
            Ok(admitted) => admitted,
            Err(error) => return Self::refused(run.host, error, run.meter),
        };
        let (mut store, meter) = self.new_store(memory, run);
        let result = {
            let work = async {
                Self::arm(&mut store, true)?;
                let linker = async_linker::<S, C>(&self.engine.engine)?;
                let instance = linker
                    .instantiate_async(&mut store, &self.module)
                    .await
                    .map_err(|e| RuntimeError::wasm("instantiate", format!("{e:#}")))?;
                store.data_mut().memory = instance.get_memory(&mut store, "memory");
                instance
                    .get_typed_func::<(), i32>(&mut store, "run")
                    .map_err(|e| RuntimeError::wasm("run", e))?
                    .call_async(&mut store, ())
                    .await
                    .map_err(|e| RuntimeError::wasm("run", format!("{e:#}")))
            };
            // Host panics are caught per poll, so the store (and the host
            // inside it) outlives them.
            let mut work = std::pin::pin!(work);
            std::future::poll_fn(|cx| {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    std::future::Future::poll(work.as_mut(), cx)
                })) {
                    Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
                    Ok(std::task::Poll::Ready(result)) => std::task::Poll::Ready(Ok(result)),
                    Err(panic) => std::task::Poll::Ready(Err(panic)),
                }
            })
            .await
        };
        let panicked = result.is_err();
        Self::finish(
            store,
            result.unwrap_or_else(|e| Err(Self::host_panic(e))),
            panicked,
            meter,
        )
    }
}

#[cfg(test)]
mod tests;
