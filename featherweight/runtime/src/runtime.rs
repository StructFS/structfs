//! The Featherweight runtime: assembly instantiation, lazy block startup,
//! server-protocol routing, lifecycle, and shutdown.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use structfs_core_store::{Error, Format, MemoryStore, Path, ReadOnly, Value};
use structfs_serde_store::MultiCodec;

use crate::assembly::{AssemblyDef, WireTarget};
use crate::block::{BlockCell, BlockId, BlockState, ShutdownMode};
use crate::core_wasm::{is_component, CoreWasmBlock};
use crate::determinism::Determinism;
use crate::error::{Result, RuntimeError};
use crate::iso::{IsoConfig, IsoSurface, LogSink, StderrLog};
use crate::metering::Metering;
use crate::namespace::{host_store, HostStore, Namespace, SessionWitness, Target, WiringTable};
use crate::native::NativeBlockFactory;
use crate::protocol::{decode_read_response, decode_write_response};
use crate::session::SessionLog;
use crate::spawn::{ProcStore, SpawnProtocol};
use crate::stdio::{HostStdio, NullStdio, Stdio};
use crate::transcript::{BlockTranscript, PreambleProfile, TranscriptMode};
use crate::turnstile::Turnstile;
use structfs_handles::CancelToken;

/// A loaded wasm artifact in some binding of the Block ABI: it serves
/// its manifest pre-wiring and runs over the block's namespace.
///
/// Binding adapters implement this to teach the runtime new artifact
/// kinds; the core knows only the Block ABI (spec 10) and its own
/// core-wasm binding (spec 11) — everything else registers through
/// [`Runtime::register_loader`].
pub trait WasmBlockDriver: Send + Sync {
    /// The block's JSON manifest (spec 01), retrieved pre-wiring.
    fn manifest(&self) -> Result<Vec<u8>>;

    /// Run the block over its namespace in the declared format; returns
    /// the guest's exit code. The code is advisory per spec 11 — a
    /// `shutdown/complete` the block wrote takes precedence.
    fn run(
        &self,
        id: BlockId,
        namespace: Namespace,
        format: Format,
        metering: &Metering,
        cancel: CancelToken,
    ) -> Result<i32>;
}

/// Recognizes and loads wasm artifacts for one binding of the Block ABI.
pub trait ArtifactLoader: Send + Sync {
    /// Whether these artifact bytes belong to this loader's binding.
    fn matches(&self, bytes: &[u8]) -> bool;

    /// Load the artifact into a runnable driver.
    fn load(&self, bytes: Vec<u8>) -> Result<Arc<dyn WasmBlockDriver>>;
}

/// The built-in loader: the core-wasm binding (spec 11). Claims any
/// artifact that is not a wasm component (core modules, and wat text in
/// tests) and runs it over the standard transports.
struct CoreWasmLoader;

impl ArtifactLoader for CoreWasmLoader {
    fn matches(&self, bytes: &[u8]) -> bool {
        !is_component(bytes)
    }

    fn load(&self, bytes: Vec<u8>) -> Result<Arc<dyn WasmBlockDriver>> {
        Ok(Arc::new(CoreWasmDriver(CoreWasmBlock::new(bytes))))
    }
}

struct CoreWasmDriver(CoreWasmBlock);

impl WasmBlockDriver for CoreWasmDriver {
    fn manifest(&self) -> Result<Vec<u8>> {
        self.0.manifest()
    }

    fn run(
        &self,
        id: BlockId,
        namespace: Namespace,
        format: Format,
        metering: &Metering,
        cancel: CancelToken,
    ) -> Result<i32> {
        self.0.run(
            id,
            namespace,
            MultiCodec::standard(),
            format,
            metering,
            cancel,
        )
    }
}

/// How a block's code is executed.
pub(crate) enum Driver {
    /// A native Rust block from the builtin registry.
    Native(Arc<dyn NativeBlockFactory>),
    /// A wasm artifact, with its declared serialization format.
    Wasm(Arc<dyn WasmBlockDriver>, Format),
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
    /// Assembly-scoped transcript identity (spec 12): `root/block`,
    /// `root/nested/block`, with `#n` appended when a key recurs across
    /// instantiations (the same definition spawned twice). Unlike
    /// `cell.id` it is stable across runs, which is what lets a
    /// recording made today replay tomorrow.
    transcript_key: String,
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
    metering: Mutex<Metering>,
    transcript_mode: Mutex<TranscriptMode>,
    determinism: Mutex<Determinism>,
    /// How many times each transcript key base has been claimed, for the
    /// `#n` suffix on reuse.
    transcript_keys: Mutex<HashMap<String, u64>>,
    /// The session log (spec 12): the assembly-wide forensic witness,
    /// when one is attached.
    session: Mutex<Option<Arc<SessionLog>>>,
    /// The deterministic scheduler, when Determinism::Simulation is on.
    turnstile: Mutex<Option<Arc<Turnstile>>>,
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

        // Under simulation, a block-thread caller parks through the
        // turnstile: the enqueue registered it against the token, the
        // callee's respond makes it runnable during the callee's turn,
        // and there is no wall-clock timeout — a wedge is a detected
        // deadlock, not a timing accident.
        if let (Some(turnstile), Some(me)) = (self.turnstile(), crate::turnstile::current_block()) {
            let rx = cell.enqueue(op, path, data);
            turnstile.park(&me);
            let response = rx.await;
            turnstile.wait_turn(&me).await;
            return match response {
                Ok(response) => Ok(response),
                Err(_) => Err(Error::overloaded("store temporarily unavailable")),
            };
        }
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

    pub(crate) fn turnstile(&self) -> Option<Arc<Turnstile>> {
        self.turnstile
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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

        // Transcripts (spec 12): open the block's transcript before its
        // code runs, and fail the start loudly if the transcript store can't be
        // had — a partial transcript is worse than no run.
        let transcript_mode = self
            .transcript_mode
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let transcript = match &transcript_mode {
            TranscriptMode::Off => None,
            TranscriptMode::Record(provider) => Some(
                provider(&block.transcript_key)
                    .map(BlockTranscript::recording)
                    .map_err(|e| {
                        cell.set_state(BlockState::Failed);
                        RuntimeError::assembly(format!(
                            "transcript store for block '{}': {e}",
                            block.transcript_key
                        ))
                    })?,
            ),
            TranscriptMode::Replay(provider) => Some(
                provider(&block.transcript_key)
                    .and_then(BlockTranscript::replaying)
                    .map_err(|e| {
                        cell.set_state(BlockState::Failed);
                        RuntimeError::assembly(format!(
                            "transcript for block '{}': {e}",
                            block.transcript_key
                        ))
                    })?,
            ),
            TranscriptMode::Seek { provider, to } => Some(
                provider(&block.transcript_key)
                    .and_then(|store| BlockTranscript::seeking(store, to(&block.transcript_key)))
                    .map_err(|e| {
                        cell.set_state(BlockState::Failed);
                        RuntimeError::assembly(format!(
                            "seek transcript for block '{}': {e}",
                            block.transcript_key
                        ))
                    })?,
            ),
        };

        // A seek's prefix must be reconstructible for the handoff to be
        // sound: effects into wired peers were suppressed during replay,
        // and a live world missing the block's own effects is refused
        // loudly rather than handed a confused block. The profile also
        // says how far to fast-forward seeded sources.
        let profile: Option<PreambleProfile> = if matches!(
            &transcript_mode,
            TranscriptMode::Seek { .. }
        ) {
            let profile = transcript
                .as_ref()
                .expect("seek builds a transcript")
                .preamble_profile();
            if let Some((index, at)) = &profile.peer_write {
                cell.set_state(BlockState::Failed);
                return Err(RuntimeError::assembly(format!(
                        "cannot seek block '{}' past entry {index}: the prefix writes                          to '{at}', an effect the live world will not hold — seek                          before it, or replay the whole run",
                        block.transcript_key
                    )));
            }
            Some(profile)
        } else {
            None
        };

        let proc = block.spawn.then(|| {
            SpawnProtocol::store(
                self.runtime.clone(),
                block.base_dir.clone(),
                self.handle.clone(),
                Some(block.wiring.clone()),
            )
        });
        // The session log (spec 12) witnesses every mode — live,
        // recording, and replay — under the block's stable key.
        let session = self
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .map(|log| SessionWitness {
                log,
                block: block.transcript_key.clone(),
            });

        // Determinism (spec 12) is the orthogonal feature: it decides how
        // the iso surface sources time and entropy, whether or not a
        // transcript is being kept.
        // Streams derive from the transcript key, not the bare name:
        // identity that is stable across runs and unique across the
        // assembly tree, so two blocks named alike never share entropy.
        let sources = self
            .determinism
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sources_for(&block.transcript_key);
        if let Some(profile) = &profile {
            sources.fast_forward(profile.entropy_words, profile.clock_ticks);
        }
        let iso = Arc::new(IsoSurface::new(IsoConfig {
            cell: block.cell.clone(),
            log: self.log_sink(),
            stdio: self.stdio_for(&block),
            env: block.env.clone(),
            args: block.args.clone(),
            proc,
            handle: self.handle.clone(),
            sources,
        }));

        let ctx = self.clone();
        let turnstile = self.turnstile();
        let sim_key = block.transcript_key.clone();
        // Enroll before the thread exists: the schedule's view of who is
        // runnable follows instantiation order, never thread-start races.
        if let Some(turnstile) = &turnstile {
            turnstile.enroll(&sim_key);
        }
        let task = self.handle.spawn_blocking(move || {
            // Under simulation this thread is a scheduled block: it
            // executes only while holding the turn, and the thread-local
            // key lets every enqueue know who is asking.
            if let Some(turnstile) = &turnstile {
                crate::turnstile::set_current_block(Some(sim_key.clone()));
                ctx.block_on(turnstile.start(&sim_key));
            }
            let mut namespace = Namespace::new(
                ctx.clone(),
                iso,
                block.wiring.clone(),
                block.cell.clone(),
                transcript,
                session,
            );

            // Spec 05 ties Running to "begins reading requests", but an
            // interactive or client-only block may never read them; the
            // strawman marks Running when the driver's code starts.
            block.cell.set_state(BlockState::Running);

            let result: std::result::Result<(), String> = match &block.driver {
                Driver::Native(factory) => {
                    let mut native = factory.create();
                    native.run(&mut namespace).map_err(|e| e.to_string())
                }
                Driver::Wasm(driver, format) => {
                    let metering = ctx
                        .metering
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let cancel = block.cell.cancel.clone();
                    driver
                        .run(
                            block.cell.id.clone(),
                            namespace,
                            format.clone(),
                            &metering,
                            cancel,
                        )
                        .map_err(|e| e.to_string())
                        .map(|code| {
                            // Spec 11: run's return value is the exit
                            // code, unless the block already declared
                            // one via shutdown/complete.
                            if code != 0 && !block.cell.shutdown_complete() {
                                block.cell.mark_shutdown_complete(code as i64);
                            }
                        })
                }
            };
            if let Some(turnstile) = &turnstile {
                turnstile.exit(&sim_key);
                crate::turnstile::set_current_block(None);
            }
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
    /// ([spec 05](https://github.com/StructFS/structfs/blob/main/isotope/spec/05-lifecycle.md)).
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
    loaders: Mutex<Vec<Arc<dyn ArtifactLoader>>>,
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

    /// Load a wasm artifact through the registered binding loaders.
    fn load_artifact(&self, artifact: &str, bytes: Vec<u8>) -> Result<Arc<dyn WasmBlockDriver>> {
        let loaders = self.loaders.lock().unwrap_or_else(|e| e.into_inner());
        for loader in loaders.iter() {
            if loader.matches(&bytes) {
                return loader.load(bytes);
            }
        }
        Err(RuntimeError::assembly(format!(
            "no registered artifact loader recognizes '{artifact}' \
             (adapters add bindings via Runtime::register_loader)"
        )))
    }

    /// Instantiate an assembly definition. See [`Runtime::instantiate`].
    ///
    /// The transcript scope for a root instantiation — whether by the
    /// embedder or a spawner — is the definition's own name; nesting
    /// appends block names below it.
    pub(crate) fn instantiate(
        self: &Arc<Self>,
        def: &AssemblyDef,
        imports: HashMap<String, HostStore>,
        base_dir: &std::path::Path,
    ) -> Result<Arc<AssemblyInstance>> {
        self.instantiate_scoped(def, imports, base_dir, &def.name)
    }

    fn instantiate_scoped(
        self: &Arc<Self>,
        def: &AssemblyDef,
        imports: HashMap<String, HostStore>,
        base_dir: &std::path::Path,
        scope: &str,
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
        let mut keys: BTreeMap<String, String> = BTreeMap::new();
        let mut children = Vec::new();

        // Claim a block's transcript key: assembly-scoped, `#n` on
        // reuse. The counter lives for the runtime, so the same
        // definition spawned twice gets `…/kv` then `…/kv#2` — in spawn
        // order, which a deterministic run makes stable. The key is also
        // the block's *identity*: `BlockId` derives from it, so
        // `iso/self/id` answers the same string on every run — an input
        // a seeded run may read without breaking the same-seed claim.
        let claim_key = |name: &str| -> String {
            let base = format!("{scope}/{name}");
            let mut keys = self
                .ctx
                .transcript_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let uses = keys.entry(base.clone()).or_insert(0);
            *uses += 1;
            match *uses {
                1 => base,
                n => format!("{base}#{n}"),
            }
        };

        // Create cells (or recurse for nested assemblies).
        for (name, block_def) in &def.blocks {
            let artifact = block_def.artifact.as_str();
            if let Some(builtin) = artifact.strip_prefix("builtin:") {
                let factory = self.lock_builtins().get(builtin).cloned().ok_or_else(|| {
                    RuntimeError::assembly(format!("unknown builtin block '{builtin}'"))
                })?;
                let key = claim_key(name);
                let mut cell = BlockCell::keyed(name.clone(), def.failure_policy(name), &key);
                if let Some(turnstile) = self.ctx.turnstile() {
                    cell.attach_simulation(&key, turnstile);
                }
                cells.insert(name.clone(), Arc::new(cell));
                keys.insert(name.clone(), key);
                drivers.insert(name.clone(), Driver::Native(factory));
            } else if artifact.ends_with(".wasm") {
                let path = base_dir.join(artifact);
                let driver = self.load_artifact(artifact, std::fs::read(&path)?)?;
                let format = wasm_format(&driver.manifest()?, block_def.serialization.as_str())?;
                let key = claim_key(name);
                let mut cell = BlockCell::keyed(name.clone(), def.failure_policy(name), &key);
                if let Some(turnstile) = self.ctx.turnstile() {
                    cell.attach_simulation(&key, turnstile);
                }
                cells.insert(name.clone(), Arc::new(cell));
                keys.insert(name.clone(), key);
                drivers.insert(name.clone(), Driver::Wasm(driver, format));
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
                // The child's scope is its position in the tree, not its
                // definition's name: two nestings of one definition must
                // not share transcript identities.
                let child_scope = format!("{scope}/{name}");
                let child =
                    self.instantiate_scoped(&child_def, HashMap::new(), base_dir, &child_scope)?;
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

            let transcript_key = keys[&name].clone();

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
                    transcript_key,
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

        // The public block starts eagerly; everything else is lazy —
        // except under simulation, where every block starts at once:
        // autonomous blocks are what concurrency means, and the seeded
        // schedule needs them all in the runnable set from the top.
        self.ctx.ensure_started(&public)?;
        if let Some(turnstile) = self.ctx.turnstile() {
            for cell in instance.cells.values() {
                self.ctx.ensure_started(cell)?;
            }
            // Every initial block is enrolled: make the first decision.
            turnstile.launch();
        }
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
                metering: Mutex::new(Metering::default()),
                transcript_mode: Mutex::new(TranscriptMode::Off),
                determinism: Mutex::new(Determinism::Live),
                transcript_keys: Mutex::new(HashMap::new()),
                session: Mutex::new(None),
                turnstile: Mutex::new(None),
                runtime: weak.clone(),
            }),
            builtins: Mutex::new(HashMap::new()),
            loaders: Mutex::new(vec![Arc::new(CoreWasmLoader)]),
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

    /// Set the transcript mode (spec 12; default: [`TranscriptMode::Off`]).
    ///
    /// `Record` executes live and appends every boundary answer to each
    /// block's transcript store; `Replay` answers every boundary operation
    /// from the transcript and never consults the live world. Transcripts are
    /// per-block, keyed by block name through the mode's provider.
    /// Orthogonal to [`Runtime::with_determinism`] — mix and match.
    pub fn with_transcripts(self, transcript_mode: TranscriptMode) -> Self {
        *self
            .inner
            .ctx
            .transcript_mode
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = transcript_mode;
        self
    }

    /// Set the determinism mode (spec 12; default: [`Determinism::Live`]).
    ///
    /// Three levels, chosen per run:
    ///
    /// - `Live` — the performance mode: real clock and entropy, blocks
    ///   fully parallel, no scheduler, and the determinism hooks cost a
    ///   `None` check.
    /// - `Seeded` — deterministic sources: two runs with one seed see
    ///   the same `/iso/time` and `/iso/random` answers; blocks still
    ///   run in parallel at full speed.
    /// - `Simulation` — Antithesis-style: `Seeded` plus the seeded
    ///   deterministic scheduler, so cross-block interleaving — racy
    ///   assemblies included — is one reproducible run per seed, with
    ///   deadlocks detected. Blocks run one turn at a time: this trades
    ///   throughput for reproducibility, which is why it is a mode and
    ///   not the default.
    ///
    /// Orthogonal to [`Runtime::with_transcripts`] — mix and match: a
    /// seeded run can be recorded, and recording a live run is a record
    /// of what happened, not a promise it can be reproduced.
    pub fn with_determinism(self, determinism: Determinism) -> Self {
        if let Some(seed) = determinism.simulation_seed() {
            let turnstile = Turnstile::new(seed);
            let runtime = Arc::downgrade(&self.inner);
            // A wedged schedule can never recover on its own (all wakes
            // happen on turns), so shut the assembly down loudly.
            turnstile.set_deadlock_handler(move || {
                if let Some(runtime) = runtime.upgrade() {
                    for block in runtime.ctx.lock_blocks().values() {
                        block
                            .cell
                            .request_shutdown(crate::block::ShutdownMode::Immediate);
                    }
                }
            });
            *self
                .inner
                .ctx
                .turnstile
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(turnstile);
        }
        *self
            .inner
            .ctx
            .determinism
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = determinism;
        self
    }

    /// Attach a session log (spec 12): an assembly-wide, arrival-order
    /// forensic witness of every block's boundary operations, written to
    /// `store` with the append-log convention. Observation-class — it
    /// answers nothing, replay never reads it, and it works in every
    /// mode: live (a flight recorder with no transcripts), recording
    /// (entries link into the transcripts), and replay (the re-run's own
    /// timeline). Orthogonal to transcripts and determinism — mix freely.
    pub fn with_session_log(self, store: HostStore) -> Self {
        *self
            .inner
            .ctx
            .session
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(SessionLog::new(store));
        self
    }

    /// Set guest metering (fuel and epoch interruption) for wasm blocks.
    /// Default: epoch interruption at 10ms, no fuel cap.
    pub fn with_metering(self, metering: Metering) -> Self {
        *self
            .inner
            .ctx
            .metering
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = metering;
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

    /// Register a binding adapter's artifact loader. Registered loaders
    /// are consulted before the built-in core-wasm loader, in
    /// registration order.
    pub fn register_loader(&mut self, loader: Arc<dyn ArtifactLoader>) {
        self.inner
            .loaders
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(0, loader);
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
fn wasm_format(manifest_bytes: &[u8], declared: &str) -> Result<Format> {
    let manifest: serde_json::Value = serde_json::from_slice(manifest_bytes)
        .map_err(|e| RuntimeError::Manifest(format!("manifest is not JSON: {e}")))?;
    let serialization = manifest
        .get("serialization")
        .and_then(|v| v.as_str())
        .unwrap_or(declared);
    match serialization {
        "application/json" => Ok(Format::JSON),
        "application/cbor" => Ok(Format::CBOR),
        "application/x-flexbuffers" => Ok(Format::FLEXBUFFERS),
        other => Err(RuntimeError::Manifest(format!(
            "unsupported serialization '{other}' (supported: application/json, \
             application/cbor, application/x-flexbuffers)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_format_accepts_every_transport() {
        for (declared, expected) in [
            ("application/json", Format::JSON),
            ("application/cbor", Format::CBOR),
            ("application/x-flexbuffers", Format::FLEXBUFFERS),
        ] {
            let manifest = format!(r#"{{"serialization": "{declared}"}}"#);
            assert_eq!(
                wasm_format(manifest.as_bytes(), "application/json").unwrap(),
                expected
            );
        }
    }

    #[test]
    fn wasm_format_falls_back_to_the_declaration() {
        assert_eq!(
            wasm_format(b"{}", "application/cbor").unwrap(),
            Format::CBOR
        );
    }

    #[test]
    fn wasm_format_rejects_unknown_transports() {
        let err = wasm_format(br#"{"serialization": "application/protobuf"}"#, "x").unwrap_err();
        assert!(err.to_string().contains("unsupported serialization"));
    }
}
