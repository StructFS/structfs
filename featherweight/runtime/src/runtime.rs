//! The Featherweight runtime: assembly instantiation, lazy block startup,
//! server-protocol routing, lifecycle, and shutdown.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use structfs_core_store::{Error, Format, MemoryStore, Path, ReadOnly, Value};
use structfs_serde_store::JsonCodec;

use crate::assembly::{AssemblyDef, WireTarget};
use crate::block::{BlockCell, BlockId, BlockState, ShutdownMode};
use crate::error::{Result, RuntimeError};
use crate::iso::{IsoConfig, IsoSurface, LogSink, StderrLog};
use crate::namespace::{host_store, HostStore, Namespace, Target, WiringTable};
use crate::native::NativeBlockFactory;
use crate::protocol::{decode_read_response, decode_write_response};
use crate::spawn::{ProcStore, SpawnProtocol};
use crate::stdio::{HostStdio, NullStdio, Stdio};
use crate::wasm_block::WasmBlock;

/// How a block's code is executed.
pub(crate) enum Driver {
    /// A native Rust block from the builtin registry.
    Native(Arc<dyn NativeBlockFactory>),
    /// A wasm component, with its declared serialization format.
    Wasm(Arc<WasmBlock>, Format),
}

/// Everything the runtime knows about one startable block.
pub(crate) struct BlockRuntime {
    pub(crate) cell: Arc<BlockCell>,
    driver: Driver,
    wiring: Arc<WiringTable>,
    /// Sibling cells in the same assembly, for fail-fast propagation.
    siblings: Vec<Arc<BlockCell>>,
    env: Arc<BTreeMap<String, String>>,
    args: Arc<Vec<String>>,
    stdio_kind: String,
    spawn: bool,
    base_dir: std::path::PathBuf,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Picks the stdio backend for a block by name; `None` falls through to
/// the block definition's `stdio` field.
pub type StdioProvider = dyn Fn(&str) -> Option<Arc<dyn Stdio>> + Send + Sync;

/// Shared runtime context: the tokio handle, the block registry, and the
/// per-operation deadline.
pub(crate) struct RtCtx {
    handle: tokio::runtime::Handle,
    // Interior mutability: RtCtx sits behind Arcs (including a Weak from
    // new_cyclic), so builder-style configuration cannot use get_mut.
    timeout: Mutex<Duration>,
    blocks: Mutex<HashMap<BlockId, Arc<BlockRuntime>>>,
    log: Mutex<Arc<dyn LogSink>>,
    stdio_provider: Mutex<Arc<StdioProvider>>,
    runtime: Weak<RuntimeInner>,
}

impl RtCtx {
    /// Run a future to completion from a blocking thread.
    pub(crate) fn block_on<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        self.handle.block_on(fut)
    }

    fn lock_blocks(&self) -> std::sync::MutexGuard<'_, HashMap<BlockId, Arc<BlockRuntime>>> {
        self.blocks.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Route a read to a block via the server protocol.
    pub(crate) async fn call_read(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        path: Path,
    ) -> std::result::Result<Option<Value>, Error> {
        let response = self.call(cell, "read", path, Value::Null).await?;
        decode_read_response(response)
    }

    /// Route a write to a block via the server protocol.
    pub(crate) async fn call_write(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        path: Path,
        data: Value,
    ) -> std::result::Result<Path, Error> {
        let response = self.call(cell, "write", path, data).await?;
        decode_write_response(response)
    }

    async fn call(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        op: &'static str,
        path: Path,
        data: Value,
    ) -> std::result::Result<Value, Error> {
        // A caller cannot tell what's behind the path: dead blocks are
        // "temporarily unavailable", nothing more.
        if cell.state().is_terminal() {
            return Err(Error::overloaded("store temporarily unavailable"));
        }
        self.ensure_started(cell)
            .map_err(|e| Error::store("runtime", "start", e.to_string()))?;

        let timeout = *self.timeout.lock().unwrap_or_else(|e| e.into_inner());
        let rx = cell.enqueue(op, path, data);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            // Sender dropped: the block reached a terminal state.
            Ok(Err(_)) => Err(Error::overloaded("store temporarily unavailable")),
            Err(_) => Err(Error::deadline_exceeded(format!(
                "no response within {timeout:?}"
            ))),
        }
    }

    fn log_sink(&self) -> Arc<dyn LogSink> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn stdio_for(&self, block: &Arc<BlockRuntime>) -> Arc<dyn Stdio> {
        let provider = self
            .stdio_provider
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(stdio) = provider(&block.cell.name) {
            return stdio;
        }
        if block.stdio_kind == "host" {
            Arc::new(HostStdio)
        } else {
            Arc::new(NullStdio)
        }
    }

    /// Start a block if it's still in `Created` (lazy startup).
    pub(crate) fn ensure_started(self: &Arc<Self>, cell: &Arc<BlockCell>) -> Result<()> {
        if !cell.try_begin_start() {
            return Ok(());
        }
        let Some(block) = self.lock_blocks().get(&cell.id).cloned() else {
            cell.set_state(BlockState::Failed);
            return Err(RuntimeError::assembly(format!(
                "no driver registered for block '{}'",
                cell.name
            )));
        };

        let proc = block.spawn.then(|| {
            SpawnProtocol::store(
                self.runtime.clone(),
                block.base_dir.clone(),
                self.handle.clone(),
                Some(block.wiring.clone()),
            )
        });
        let iso = Arc::new(IsoSurface::new(IsoConfig {
            cell: block.cell.clone(),
            log: self.log_sink(),
            stdio: self.stdio_for(&block),
            env: block.env.clone(),
            args: block.args.clone(),
            proc,
            handle: self.handle.clone(),
        }));

        let ctx = self.clone();
        let task = self.handle.spawn_blocking(move || {
            let mut namespace =
                Namespace::new(ctx.clone(), iso, block.wiring.clone(), block.cell.clone());

            // Spec 05 ties Running to "begins reading requests", but an
            // interactive or client-only block may never read them; the
            // strawman marks Running when the driver's code starts.
            block.cell.set_state(BlockState::Running);

            let result: std::result::Result<(), String> = match &block.driver {
                Driver::Native(factory) => {
                    let mut native = factory.create();
                    native.run(&mut namespace).map_err(|e| e.to_string())
                }
                Driver::Wasm(wasm, format) => wasm
                    .run(block.cell.id.clone(), namespace, JsonCodec, format.clone())
                    .map_err(|e| e.to_string()),
            };
            finalize(&block, result);
        });
        if let Some(entry) = self.lock_blocks().get(&cell.id) {
            *entry.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        }
        Ok(())
    }
}

/// Record a finished driver run on the cell and apply the failure policy.
fn finalize(block: &BlockRuntime, result: std::result::Result<(), String>) {
    match result {
        Ok(()) => block.cell.set_state(BlockState::Stopped),
        Err(message) => {
            if block.cell.shutdown_requested() {
                // Errors while tearing down (cancelled parked reads) are
                // intentional termination, not failure.
                block.cell.set_state(BlockState::Stopped);
            } else {
                block.cell.record_error(message);
                block.cell.set_state(BlockState::Failed);
                if block.cell.failure == crate::block::FailurePolicy::FailFast {
                    for sibling in &block.siblings {
                        if sibling.id != block.cell.id {
                            sibling.request_shutdown(ShutdownMode::Graceful);
                        }
                    }
                }
            }
        }
    }
}

/// A running (or runnable) assembly.
pub struct AssemblyInstance {
    /// The assembly's name from its definition.
    pub name: String,
    ctx: Arc<RtCtx>,
    cells: BTreeMap<String, Arc<BlockCell>>,
    public: Arc<BlockCell>,
    children: Vec<Arc<AssemblyInstance>>,
}

impl std::fmt::Debug for AssemblyInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("AssemblyInstance");
        dbg.field("name", &self.name);
        for (name, cell) in &self.cells {
            dbg.field(name, &cell.state().as_str());
        }
        dbg.finish()
    }
}

impl AssemblyInstance {
    /// The public block's cell — the assembly's identity from outside.
    pub fn public_cell(&self) -> &Arc<BlockCell> {
        &self.public
    }

    /// Look up a block cell by local name.
    pub fn cell(&self, name: &str) -> Option<&Arc<BlockCell>> {
        self.cells.get(name)
    }

    /// Read from the assembly's store (its public block).
    pub async fn read(&self, path: Path) -> std::result::Result<Option<Value>, Error> {
        self.ctx.call_read(&self.public, path).await
    }

    /// Write to the assembly's store (its public block).
    pub async fn write(&self, path: Path, data: Value) -> std::result::Result<Path, Error> {
        self.ctx.call_write(&self.public, path, data).await
    }

    /// Park until the public block reaches a terminal state.
    pub async fn wait_public_terminal(&self) {
        self.public.wait_terminal().await
    }

    /// Deliver a signal to a named block's mailbox.
    pub fn signal(&self, block: &str, name: impl Into<String>, data: Value) -> bool {
        match self.cells.get(block) {
            Some(cell) => {
                cell.deliver_signal(name, data);
                true
            }
            None => false,
        }
    }

    /// Host escape hatch: read from a named internal block's store.
    ///
    /// Blocks cannot see each other except through wiring; the embedding
    /// host can (for routing and diagnostics, like a gateway mounting
    /// blocks behind HTTP routes).
    pub async fn read_block(
        &self,
        name: &str,
        path: Path,
    ) -> std::result::Result<Option<Value>, Error> {
        let cell = self
            .cells
            .get(name)
            .ok_or_else(|| Error::store("assembly", "read_block", format!("no block '{name}'")))?;
        self.ctx.call_read(cell, path).await
    }

    /// Host escape hatch: write to a named internal block's store.
    pub async fn write_block(
        &self,
        name: &str,
        path: Path,
        data: Value,
    ) -> std::result::Result<Path, Error> {
        let cell = self
            .cells
            .get(name)
            .ok_or_else(|| Error::store("assembly", "write_block", format!("no block '{name}'")))?;
        self.ctx.call_write(cell, path, data).await
    }

    fn all_cells(&self) -> Vec<Arc<BlockCell>> {
        let mut cells: Vec<_> = self.cells.values().cloned().collect();
        for child in &self.children {
            cells.extend(child.all_cells());
        }
        cells
    }

    /// Synchronously request graceful shutdown of every block. Parked
    /// mailbox reads unblock immediately; use [`AssemblyInstance::shutdown`]
    /// to also wait and escalate.
    pub fn request_shutdown(&self) {
        for cell in self.all_cells() {
            cell.request_shutdown(ShutdownMode::Graceful);
        }
    }

    /// Shut the assembly down: graceful first, escalating to immediate
    /// for blocks that don't stop within `timeout`
    /// (`isotope/spec/05-lifecycle.md`).
    pub async fn shutdown(&self, timeout: Duration) {
        let cells = self.all_cells();
        for cell in &cells {
            cell.request_shutdown(ShutdownMode::Graceful);
        }
        for cell in &cells {
            if tokio::time::timeout(timeout, cell.wait_terminal())
                .await
                .is_err()
            {
                cell.request_shutdown(ShutdownMode::Immediate);
                let _ = tokio::time::timeout(Duration::from_secs(1), cell.wait_terminal()).await;
            }
        }
    }
}

/// The shared core of a [`Runtime`], referenced by spawn/management
/// stores so blocks can instantiate assemblies through the store surface.
pub struct RuntimeInner {
    ctx: Arc<RtCtx>,
    builtins: Mutex<HashMap<String, Arc<dyn NativeBlockFactory>>>,
}

impl RuntimeInner {
    /// The shared runtime context (for grant stores and spawn surfaces).
    pub(crate) fn ctx(&self) -> Arc<RtCtx> {
        self.ctx.clone()
    }

    fn lock_builtins(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, Arc<dyn NativeBlockFactory>>> {
        self.builtins.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Instantiate an assembly definition. See [`Runtime::instantiate`].
    pub(crate) fn instantiate(
        self: &Arc<Self>,
        def: &AssemblyDef,
        imports: HashMap<String, HostStore>,
        base_dir: &std::path::Path,
    ) -> Result<Arc<AssemblyInstance>> {
        for import in def.imports.keys() {
            if !imports.contains_key(import) {
                return Err(RuntimeError::assembly(format!(
                    "assembly '{}' requires import '${}': {}",
                    def.name, import, def.imports[import]
                )));
            }
        }

        let mut cells: BTreeMap<String, Arc<BlockCell>> = BTreeMap::new();
        let mut drivers: BTreeMap<String, Driver> = BTreeMap::new();
        let mut children = Vec::new();

        // Create cells (or recurse for nested assemblies).
        for (name, block_def) in &def.blocks {
            let artifact = block_def.artifact.as_str();
            if let Some(builtin) = artifact.strip_prefix("builtin:") {
                let factory = self.lock_builtins().get(builtin).cloned().ok_or_else(|| {
                    RuntimeError::assembly(format!("unknown builtin block '{builtin}'"))
                })?;
                cells.insert(
                    name.clone(),
                    Arc::new(BlockCell::new(name.clone(), def.failure_policy(name))),
                );
                drivers.insert(name.clone(), Driver::Native(factory));
            } else if artifact.ends_with(".wasm") {
                let path = base_dir.join(artifact);
                let wasm = WasmBlock::from_file(&path)?;
                let format = wasm_format(&wasm, block_def.serialization.as_str())?;
                cells.insert(
                    name.clone(),
                    Arc::new(BlockCell::new(name.clone(), def.failure_policy(name))),
                );
                drivers.insert(name.clone(), Driver::Wasm(Arc::new(wasm), format));
            } else if artifact.ends_with(".json")
                || artifact.ends_with(".yaml")
                || artifact.ends_with(".yml")
            {
                // The fractal property: a nested assembly is a block. Its
                // public cell serves as this block's cell.
                let source = std::fs::read_to_string(base_dir.join(artifact))?;
                let child_def = AssemblyDef::from_str(&source)?;
                if !child_def.imports.is_empty() {
                    return Err(RuntimeError::assembly(format!(
                        "nested assembly '{}' declares imports; binding parent \
                         wiring to child imports is not supported in the strawman",
                        child_def.name
                    )));
                }
                let child = self.instantiate(&child_def, HashMap::new(), base_dir)?;
                cells.insert(name.clone(), child.public_cell().clone());
                children.push(child);
            } else {
                return Err(RuntimeError::assembly(format!(
                    "unsupported artifact reference '{artifact}' \
                     (expected builtin:{{name}}, *.wasm, or a nested *.json/*.yaml)"
                )));
            }
        }

        let public = cells
            .get(&def.public)
            .expect("validated by AssemblyDef")
            .clone();
        let siblings: Vec<Arc<BlockCell>> = cells.values().cloned().collect();

        // Build each startable block's wiring and register its runtime.
        for (name, driver) in drivers {
            let cell = cells[&name].clone();
            let block_def = &def.blocks[&name];
            let mut entries: Vec<(Path, Target)> = Vec::new();

            // Config appears read-only at /config (spec 02).
            if let Some(config) = def.config.get(&name) {
                let store = ReadOnly::new(MemoryStore::with_root(config.clone()));
                entries.push((
                    Path::parse("config").unwrap(),
                    Target::Store(host_store(store)),
                ));
            }
            for wire in def.wiring.iter().filter(|w| w.block == name) {
                let target = match &wire.target {
                    WireTarget::Block(target_name) => Target::Block(cells[target_name].clone()),
                    WireTarget::Import(import) => Target::Store(imports[import].clone()),
                };
                entries.push((wire.prefix.clone(), target));
            }

            self.ctx.lock_blocks().insert(
                cell.id.clone(),
                Arc::new(BlockRuntime {
                    cell: cell.clone(),
                    driver,
                    wiring: Arc::new(WiringTable::new(entries)),
                    siblings: siblings.clone(),
                    env: Arc::new(block_def.env.clone()),
                    args: Arc::new(block_def.args.clone()),
                    stdio_kind: block_def.stdio.clone(),
                    spawn: block_def.spawn,
                    base_dir: base_dir.to_path_buf(),
                    task: Mutex::new(None),
                }),
            );
        }

        let instance = Arc::new(AssemblyInstance {
            name: def.name.clone(),
            ctx: self.ctx.clone(),
            cells,
            public: public.clone(),
            children,
        });

        // The public block starts eagerly; everything else is lazy.
        self.ctx.ensure_started(&public)?;
        Ok(instance)
    }
}

/// The Featherweight runtime.
///
/// Holds the builtin native-block registry and the shared context. Blocks
/// run on blocking threads of the provided tokio runtime.
pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

impl Runtime {
    /// Create a runtime on the current tokio runtime handle.
    ///
    /// Must be called within a tokio runtime (e.g. inside `block_on` or a
    /// `#[tokio::main]`); use [`Runtime::with_handle`] otherwise.
    pub fn new() -> Self {
        Self::with_handle(tokio::runtime::Handle::current())
    }

    /// Create a runtime on an explicit tokio handle.
    pub fn with_handle(handle: tokio::runtime::Handle) -> Self {
        let inner = Arc::new_cyclic(|weak: &Weak<RuntimeInner>| RuntimeInner {
            ctx: Arc::new(RtCtx {
                handle,
                timeout: Mutex::new(Duration::from_secs(30)),
                blocks: Mutex::new(HashMap::new()),
                log: Mutex::new(Arc::new(StderrLog)),
                stdio_provider: Mutex::new(Arc::new(|_| None)),
                runtime: weak.clone(),
            }),
            builtins: Mutex::new(HashMap::new()),
        });
        Self { inner }
    }

    /// Set the per-operation deadline for routed calls (default 30s).
    ///
    /// A parked handle read can legitimately outlast this; callers of such
    /// paths should use handles rather than long synchronous calls.
    pub fn with_timeout(self, timeout: Duration) -> Self {
        *self
            .inner
            .ctx
            .timeout
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = timeout;
        self
    }

    /// Replace the log sink (default: stderr).
    pub fn with_log_sink(self, log: Arc<dyn LogSink>) -> Self {
        *self.inner.ctx.log.lock().unwrap_or_else(|e| e.into_inner()) = log;
        self
    }

    /// Override stdio selection by block name (checked before the block
    /// definition's `stdio` field). Used by tests and embedders.
    pub fn with_stdio_provider(self, provider: Arc<StdioProvider>) -> Self {
        *self
            .inner
            .ctx
            .stdio_provider
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = provider;
        self
    }

    /// Register a native block under `builtin:{name}`.
    pub fn register_builtin(
        &mut self,
        name: impl Into<String>,
        factory: Arc<dyn NativeBlockFactory>,
    ) {
        self.inner.lock_builtins().insert(name.into(), factory);
    }

    /// Instantiate an assembly definition.
    ///
    /// `imports` provides stores for the definition's declared imports;
    /// `base_dir` resolves relative artifact paths (wasm files, nested
    /// definitions).
    pub fn instantiate(
        &self,
        def: &AssemblyDef,
        imports: HashMap<String, HostStore>,
        base_dir: &std::path::Path,
    ) -> Result<Arc<AssemblyInstance>> {
        self.inner.instantiate(def, imports, base_dir)
    }

    /// The runtime's management surface, as a store (spec 08: the
    /// management API is StructFS).
    ///
    /// Writing an assembly definition (as a Value) instantiates it and
    /// returns `outstanding/{id}`; reading the handle returns status;
    /// reading `outstanding/{id}/wait` parks until terminal; a Null write
    /// shuts the assembly down. This is the same protocol blocks with the
    /// `spawn` grant see at `iso/proc`.
    pub fn management_store(&self, base_dir: &std::path::Path) -> ProcStore {
        SpawnProtocol::store(
            Arc::downgrade(&self.inner),
            base_dir.to_path_buf(),
            self.inner.ctx.handle.clone(),
            // The host has no block namespace; grants are a spawner
            // concept.
            None,
        )
    }
}

impl Default for Runtime {
    /// Equivalent to [`Runtime::new`]; requires a current tokio runtime.
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve a wasm block's serialization format from its manifest, falling
/// back to the assembly declaration. This closes the manifest bootstrap
/// loop: the codec is selected before the store bridge exists.
fn wasm_format(wasm: &WasmBlock, declared: &str) -> Result<Format> {
    let manifest_bytes = wasm.manifest()?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| RuntimeError::Manifest(format!("manifest is not JSON: {e}")))?;
    let serialization = manifest
        .get("serialization")
        .and_then(|v| v.as_str())
        .unwrap_or(declared);
    if serialization != "application/json" {
        return Err(RuntimeError::Manifest(format!(
            "unsupported serialization '{serialization}' (strawman supports application/json)"
        )));
    }
    Ok(Format::JSON)
}
