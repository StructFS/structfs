//! The Featherweight runtime: assembly instantiation, lazy block startup,
//! server-protocol routing, lifecycle, and shutdown.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use structfs_core_store::{Error, Format, MemoryStore, Path, ReadOnly, Value};
use structfs_serde_store::JsonCodec;

use crate::assembly::{AssemblyDef, WireTarget};
use crate::block::{BlockCell, BlockId, BlockState, ShutdownMode};
use crate::error::{Result, RuntimeError};
use crate::iso::{IsoSurface, LogSink, StderrLog};
use crate::namespace::{host_store, HostStore, Namespace, Target, WiringTable};
use crate::native::NativeBlockFactory;
use crate::protocol::{decode_read_response, decode_write_response};
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
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Shared runtime context: the tokio handle, the block registry, and the
/// per-operation deadline.
pub(crate) struct RtCtx {
    handle: tokio::runtime::Handle,
    timeout: Duration,
    blocks: Mutex<HashMap<BlockId, Arc<BlockRuntime>>>,
    log: Arc<dyn LogSink>,
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

        let rx = cell.enqueue(op, path, data);
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            // Sender dropped: the block reached a terminal state.
            Ok(Err(_)) => Err(Error::overloaded("store temporarily unavailable")),
            Err(_) => Err(Error::deadline_exceeded(format!(
                "no response within {:?}",
                self.timeout
            ))),
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

        let ctx = self.clone();
        let task = self.handle.spawn_blocking(move || {
            let iso = Arc::new(IsoSurface::new(block.cell.clone(), ctx.log.clone()));
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

/// The Featherweight runtime.
///
/// Holds the builtin native-block registry and the shared context. Blocks
/// run on blocking threads of the provided tokio runtime.
pub struct Runtime {
    ctx: Arc<RtCtx>,
    builtins: HashMap<String, Arc<dyn NativeBlockFactory>>,
}

impl Runtime {
    /// Create a runtime on the current tokio runtime handle.
    ///
    /// Must be called within a tokio runtime (e.g. inside `block_on` or a
    /// `#[tokio::main]`); use [`Runtime::with_handle`] otherwise.
    pub fn new() -> Self {
        Self::with_handle(tokio::runtime::Handle::current())
    }
}

impl Default for Runtime {
    /// Equivalent to [`Runtime::new`]; requires a current tokio runtime.
    fn default() -> Self {
        Self::new()
    }
}

impl Runtime {
    /// Create a runtime on an explicit tokio handle.
    pub fn with_handle(handle: tokio::runtime::Handle) -> Self {
        Self {
            ctx: Arc::new(RtCtx {
                handle,
                timeout: Duration::from_secs(30),
                blocks: Mutex::new(HashMap::new()),
                log: Arc::new(StderrLog),
            }),
            builtins: HashMap::new(),
        }
    }

    /// Set the per-operation deadline for routed calls (default 30s).
    ///
    /// A parked handle read can legitimately outlast this; callers of such
    /// paths should use handles rather than long synchronous calls.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        let ctx = Arc::get_mut(&mut self.ctx).expect("configure before instantiating");
        ctx.timeout = timeout;
        self
    }

    /// Replace the log sink (default: stderr).
    pub fn with_log_sink(mut self, log: Arc<dyn LogSink>) -> Self {
        let ctx = Arc::get_mut(&mut self.ctx).expect("configure before instantiating");
        ctx.log = log;
        self
    }

    /// Register a native block under `builtin:{name}`.
    pub fn register_builtin(
        &mut self,
        name: impl Into<String>,
        factory: Arc<dyn NativeBlockFactory>,
    ) {
        self.builtins.insert(name.into(), factory);
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
                let factory = self.builtins.get(builtin).ok_or_else(|| {
                    RuntimeError::assembly(format!("unknown builtin block '{builtin}'"))
                })?;
                cells.insert(
                    name.clone(),
                    Arc::new(BlockCell::new(name.clone(), def.failure_policy(name))),
                );
                drivers.insert(name.clone(), Driver::Native(factory.clone()));
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
