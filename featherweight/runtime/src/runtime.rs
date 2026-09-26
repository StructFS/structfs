//! The Featherweight runtime: assembly instantiation, lazy block startup,
//! server-protocol routing, lifecycle, and shutdown.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use structfs_core_store::{path, Error, Format, MemoryStore, Path, ReadOnly, Record, Value};
use structfs_serde_store::MultiCodec;

use crate::assembly::{AssemblyDef, WireTarget};
use crate::block::{BlockCell, BlockId, BlockState, BlockView, ShutdownMode};
use crate::core_wasm::{is_component, CoreWasmBlock, CoreWasmEngine, HostRun};
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
use crate::transcript::{BlockTranscript, TranscriptMode};
use crate::turnstile::Turnstile;

/// A loaded wasm artifact in some binding of the Block ABI: it serves
/// its manifest pre-wiring and runs over the block's namespace.
///
/// Binding adapters implement this to teach the runtime new artifact
/// kinds; the core knows only the Block ABI (spec 10) and its own
/// core-wasm binding (spec 11) — everything else registers through
/// [`RuntimeConfig::register_loader`] or [`RuntimeConfig::register_artifact`].
pub trait WasmBlockDriver: Send + Sync + 'static {
    /// Execute with the full lifecycle, cancellation and accounting context.
    /// Adapters must implement this directly; there is no implicit blocking bridge.
    fn execute(
        self: Arc<Self>,
        context: crate::DriverContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i32>> + Send>>;

    /// The block's JSON manifest, retrieved before wiring.
    fn manifest(&self) -> Result<Vec<u8>>;
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
/// tests). Every artifact it loads shares one engine — and one epoch
/// ticker — per runtime.
struct CoreWasmLoader {
    engine: std::sync::OnceLock<Arc<CoreWasmEngine>>,
}

impl ArtifactLoader for CoreWasmLoader {
    fn matches(&self, bytes: &[u8]) -> bool {
        !is_component(bytes)
    }

    fn load(&self, bytes: Vec<u8>) -> Result<Arc<dyn WasmBlockDriver>> {
        let engine = match self.engine.get() {
            Some(engine) => engine.clone(),
            None => {
                let engine = CoreWasmEngine::new(1)?;
                self.engine.get_or_init(|| engine).clone()
            }
        };
        Ok(Arc::new(CoreWasmDriver(Arc::new(
            engine.prepare_blocking(&bytes)?,
        ))))
    }
}

struct CoreWasmDriver(Arc<CoreWasmBlock>);

impl WasmBlockDriver for CoreWasmDriver {
    fn execute(
        self: Arc<Self>,
        context: crate::DriverContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i32>> + Send>> {
        Box::pin(async move {
            let policy = crate::ExecutionPolicy {
                fuel: context.metering.fuel,
                ..Default::default()
            };
            self.0
                .run_host_async(HostRun {
                    host: context.namespace,
                    codec: MultiCodec::standard(),
                    format: context.format,
                    policy,
                    cancel: context.cancel,
                    meter: context.usage,
                })
                .await
                .result
        })
    }

    fn manifest(&self) -> Result<Vec<u8>> {
        Ok(self.0.manifest().to_vec())
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
    provider_owner: structfs_service::OwnerHandle,
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

/// Everything a [`Runtime`] is configured with, fixed before it exists.
///
/// Build one with [`RuntimeConfig::new`] and the `with_*` methods, add
/// registrations with the `register_*` methods, then hand it to
/// [`Runtime::new`]. A running runtime cannot be reconfigured: every
/// block of every assembly it instantiates sees the same configuration.
pub struct RuntimeConfig {
    handle: tokio::runtime::Handle,
    timeout: Duration,
    call_budget: Arc<crate::admission::CallBudget>,
    execution: Option<crate::execution::ExecutionScope>,
    log: Arc<dyn LogSink>,
    stdio_provider: Arc<StdioProvider>,
    metering: Metering,
    transcripts: TranscriptMode,
    determinism: Determinism,
    session: Option<HostStore>,
    core_engine: Option<Arc<CoreWasmEngine>>,
    builtins: HashMap<String, Arc<dyn NativeBlockFactory>>,
    loaders: Vec<Arc<dyn ArtifactLoader>>,
    artifacts: HashMap<String, Arc<dyn WasmBlockDriver>>,
}

impl RuntimeConfig {
    /// Defaults: a 30 s per-operation deadline, an unbounded shared call
    /// budget, stderr logging, no transcripts, live determinism, no fuel
    /// cap, and no registrations. Blocks run on `handle`'s runtime.
    pub fn new(handle: tokio::runtime::Handle) -> Self {
        Self {
            handle,
            timeout: Duration::from_secs(30),
            call_budget: crate::admission::CallBudget::shared(Default::default()),
            execution: None,
            log: Arc::new(StderrLog),
            stdio_provider: Arc::new(|_| None),
            metering: Metering::default(),
            transcripts: TranscriptMode::Off,
            determinism: Determinism::Live,
            session: None,
            core_engine: None,
            builtins: HashMap::new(),
            loaders: Vec::new(),
            artifacts: HashMap::new(),
        }
    }

    /// Set the per-operation deadline for routed calls (default 30s).
    ///
    /// A parked handle read can legitimately outlast this; callers of such
    /// paths should use handles rather than long synchronous calls.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Share a live call budget across request runtimes; saturated calls
    /// fail immediately as Overloaded. For per-request limits, pass a child
    /// of the shared tenant/global budget. Update limits on the retained
    /// budget handle rather than replacing it.
    pub fn with_call_budget(mut self, budget: Arc<crate::admission::CallBudget>) -> Self {
        self.call_budget = budget;
        self
    }

    /// Apply one absolute deadline/cancellation scope to all blocks, routed
    /// calls and providers in this runtime. Use a fresh runtime per request.
    pub fn with_execution_scope(mut self, scope: crate::execution::ExecutionScope) -> Self {
        self.execution = Some(scope);
        self
    }

    /// Replace the log sink (default: stderr).
    pub fn with_log_sink(mut self, log: Arc<dyn LogSink>) -> Self {
        self.log = log;
        self
    }

    /// Override stdio selection by block name (checked before the block
    /// definition's `stdio` field). Used by tests and embedders.
    pub fn with_stdio_provider(mut self, provider: Arc<StdioProvider>) -> Self {
        self.stdio_provider = provider;
        self
    }

    /// Set per-run guest metering (the fuel cap) for wasm blocks. Epoch
    /// interruption is an engine concern and always on.
    pub fn with_metering(mut self, metering: Metering) -> Self {
        self.metering = metering;
        self
    }

    /// Set the transcript mode (spec 12; default: [`TranscriptMode::Off`]).
    ///
    /// `Record` executes live and appends every boundary answer to each
    /// block's transcript store; `Replay` answers every boundary operation
    /// from the transcript and never consults the live world. Transcripts are
    /// per-block, keyed by block name through the mode's provider.
    /// Orthogonal to [`RuntimeConfig::with_determinism`] — mix and match.
    pub fn with_transcripts(mut self, transcripts: TranscriptMode) -> Self {
        self.transcripts = transcripts;
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
    ///   not the default. Deadlock detection exists only in this mode.
    ///
    /// Orthogonal to [`RuntimeConfig::with_transcripts`] — mix and match: a
    /// seeded run can be recorded, and recording a live run is a record
    /// of what happened, not a promise it can be reproduced.
    pub fn with_determinism(mut self, determinism: Determinism) -> Self {
        self.determinism = determinism;
        self
    }

    /// Attach a session log (spec 12): an assembly-wide, arrival-order
    /// forensic witness of every block's boundary operations, written to
    /// `store` with the append-log convention. Observation-class — it
    /// answers nothing, replay never reads it, and it works in every
    /// mode: live (a flight recorder with no transcripts), recording
    /// (entries link into the transcripts), and replay (the re-run's own
    /// timeline). Orthogonal to transcripts and determinism — mix freely.
    pub fn with_session_log(mut self, store: HostStore) -> Self {
        self.session = Some(store);
        self
    }

    /// Share one core-wasm engine with the built-in `.wasm` loader (and
    /// with other runtimes). Without this, the runtime creates one engine
    /// on first use (`CoreWasmEngine::new(1)`: at most 10,000 concurrent
    /// core-wasm runs and 64 MiB per store) and shares it across every
    /// artifact it loads. A block that starts when every slot is taken
    /// waits, within its execution scope, until one frees.
    pub fn with_core_engine(mut self, engine: Arc<CoreWasmEngine>) -> Self {
        self.core_engine = Some(engine);
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

    /// Register a binding adapter's artifact loader. Registered loaders
    /// are consulted before the built-in core-wasm loader, most recently
    /// registered first.
    pub fn register_loader(&mut self, loader: Arc<dyn ArtifactLoader>) {
        self.loaders.insert(0, loader);
    }

    /// Register an already-prepared artifact from any external adapter
    /// under an assembly artifact identifier. The host controls
    /// preparation concurrency, caching and artifact identity.
    pub fn register_artifact(&mut self, name: impl Into<String>, driver: Arc<dyn WasmBlockDriver>) {
        self.artifacts.insert(name.into(), driver);
    }

    /// Register host-resolved core code under an assembly artifact identifier.
    /// Share a prepared block across fresh session runtimes with this method.
    pub fn register_core_artifact(&mut self, name: impl Into<String>, block: Arc<CoreWasmBlock>) {
        self.register_artifact(name, Arc::new(CoreWasmDriver(block)));
    }
}

/// Shared runtime context: the frozen configuration, the block registry,
/// and the deterministic scheduler when simulation is on.
pub(crate) struct RtCtx {
    handle: tokio::runtime::Handle,
    cleanup: Arc<structfs_service::CleanupSupervisor>,
    timeout: Duration,
    call_budget: Arc<crate::admission::CallBudget>,
    execution: Option<crate::execution::ExecutionScope>,
    log: Arc<dyn LogSink>,
    stdio_provider: Arc<StdioProvider>,
    metering: Metering,
    transcript_mode: TranscriptMode,
    determinism: Determinism,
    /// The session log (spec 12): the assembly-wide forensic witness,
    /// when one is attached.
    session: Option<Arc<SessionLog>>,
    /// The deterministic scheduler, when Determinism::Simulation is on.
    turnstile: Option<Arc<Turnstile>>,
    /// Ordered, so every walk of the registry (deadlock handling included)
    /// happens in an order that is a function of the assembly's shape.
    blocks: Mutex<BTreeMap<BlockId, Arc<BlockRuntime>>>,
    /// How many times each transcript key base has been claimed, for the
    /// `#n` suffix on reuse.
    transcript_keys: Mutex<HashMap<String, u64>>,
    runtime: Weak<RuntimeInner>,
}

/// Provider owners for each assembly bound this many resources.
const PROVIDER_RESOURCES: usize = 65536;

impl RtCtx {
    pub(crate) fn cleanup(&self) -> &Arc<structfs_service::CleanupSupervisor> {
        &self.cleanup
    }

    /// Run a future to completion from a blocking thread (native blocks).
    pub(crate) fn block_on<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        self.handle.block_on(fut)
    }

    fn lock_blocks(&self) -> std::sync::MutexGuard<'_, BTreeMap<BlockId, Arc<BlockRuntime>>> {
        self.blocks.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn execution_scope(&self) -> Option<crate::execution::ExecutionScope> {
        self.execution.clone()
    }

    pub(crate) fn turnstile(&self) -> Option<&Arc<Turnstile>> {
        self.turnstile.as_ref()
    }

    /// Route a read to a block via the server protocol.
    pub(crate) async fn call_read(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        path: Path,
    ) -> std::result::Result<Option<Value>, Error> {
        decode_read_response(self.call(cell, "read", path, Value::Null, None).await?)
    }

    /// Route a write to a block via the server protocol.
    pub(crate) async fn call_write(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        path: Path,
        data: Value,
    ) -> std::result::Result<Path, Error> {
        decode_write_response(self.call(cell, "write", path, data, None).await?)
    }

    /// One server-protocol call, inside the runtime's execution scope when
    /// it has one. `prepaid` is an admission already charged by the router;
    /// otherwise the call is charged to the shared budget here.
    pub(crate) async fn call(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        op: &'static str,
        path: Path,
        data: Value,
        prepaid: Option<structfs_service::Lease>,
    ) -> std::result::Result<Value, Error> {
        let live = self.call_live(cell, op, path, data, prepaid);
        match self.execution_scope() {
            Some(scope) => scope.run(live).await,
            None => live.await,
        }
    }

    pub(crate) fn admit_route(
        &self,
        key: &BlockId,
        path: &Path,
        data: Option<&Record>,
    ) -> std::result::Result<structfs_service::Lease, Error> {
        self.call_budget.acquire_record(key, path, data)
    }

    async fn call_live(
        self: &Arc<Self>,
        cell: &Arc<BlockCell>,
        op: &'static str,
        path: Path,
        data: Value,
        prepaid: Option<structfs_service::Lease>,
    ) -> std::result::Result<Value, Error> {
        // A caller cannot tell what's behind the path: dead blocks are
        // "temporarily unavailable", nothing more.
        if cell.state().is_terminal() {
            return Err(Error::overloaded("store temporarily unavailable"));
        }
        let charge = match prepaid {
            Some(lease) => lease,
            None => self.call_budget.acquire(&cell.admission_id, &path, &data)?,
        };
        self.ensure_started(cell)
            .map_err(|e| Error::store("runtime", "start", e.to_string()))?;

        // Under simulation, a block-thread caller parks through the
        // turnstile: the enqueue registered it against the token, the
        // callee's respond makes it runnable during the callee's turn,
        // and there is no wall-clock timeout — a wedge is a detected
        // deadlock, not a timing accident.
        if let (Some(turnstile), Some(me)) = (self.turnstile(), crate::turnstile::current_block()) {
            let rx = cell.enqueue_owned(op, path, data, charge);
            // A dependency on the callee: this park arms deadlock.
            turnstile.park(&me, crate::turnstile::ParkKind::Call);
            let response = rx.await;
            turnstile.wait_turn(&me).await;
            return response.map_err(|_| Error::overloaded("store temporarily unavailable"));
        }
        let timeout = self.timeout;
        let rx = cell.enqueue_owned(op, path, data, charge);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => Ok(response),
            // Sender dropped: the block reached a terminal state.
            Ok(Err(_)) => Err(Error::overloaded("store temporarily unavailable")),
            Err(_) => Err(Error::deadline_exceeded(format!(
                "no response within {timeout:?}"
            ))),
        }
    }

    fn stdio_for(&self, block: &BlockRuntime) -> Arc<dyn Stdio> {
        if let Some(stdio) = (self.stdio_provider)(&block.cell.name) {
            return stdio;
        }
        if block.stdio_kind == "host" {
            Arc::new(HostStdio)
        } else {
            Arc::new(NullStdio)
        }
    }

    /// Open the block's transcript store (spec 12). The provider is
    /// synchronous and runs here, so a missing store fails the start; the
    /// transcript's contents are loaded asynchronously by the block's task.
    fn transcript_source(&self, block: &BlockRuntime) -> Result<Option<TranscriptSource>> {
        let key = &block.transcript_key;
        let opened = match &self.transcript_mode {
            TranscriptMode::Off => return Ok(None),
            TranscriptMode::Record(provider) => provider(key)
                .map(TranscriptSource::Record)
                .map_err(|e| format!("transcript store for block '{key}': {e}")),
            TranscriptMode::Replay(provider) => provider(key)
                .map(TranscriptSource::Replay)
                .map_err(|e| format!("transcript for block '{key}': {e}")),
            TranscriptMode::Seek { provider, to } => provider(key)
                .map(|store| TranscriptSource::Seek(store, to(key)))
                .map_err(|e| format!("seek transcript for block '{key}': {e}")),
        };
        opened.map(Some).map_err(RuntimeError::Assembly)
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
        let transcript = match self.transcript_source(&block) {
            Ok(source) => source,
            Err(error) => {
                cell.set_state(BlockState::Failed);
                return Err(error);
            }
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
        let session = self.session.clone().map(|log| SessionWitness {
            log,
            block: block.transcript_key.clone(),
        });
        let iso = IsoConfig {
            cell: block.cell.clone(),
            log: self.log.clone(),
            stdio: self.stdio_for(&block),
            env: block.env.clone(),
            args: block.args.clone(),
            proc,
            handle: self.handle.clone(),
            // Determinism (spec 12) is the orthogonal feature: it decides
            // how the iso surface sources time and entropy, whether or not
            // a transcript is being kept. Streams derive from the
            // transcript key, not the bare name: identity that is stable
            // across runs and unique across the assembly tree.
            sources: self.determinism.sources_for(&block.transcript_key),
            capabilities: block.wiring.prefixes().map(Path::to_string).collect(),
            calls: self.call_budget.clone(),
            execution: self.execution_scope(),
        };

        let sim_key = block.transcript_key.clone();
        // Enroll before the task exists: the schedule's view of who is
        // runnable follows instantiation order, never task-start races.
        if let Some(turnstile) = &self.turnstile {
            turnstile.enroll(&sim_key);
        }
        let identity = self.turnstile.as_ref().map(|_| sim_key.clone());
        let run = run_block(self.clone(), block.clone(), iso, transcript, session);
        // The handle is stored in the same critical section that spawns the
        // task, so shutdown can never observe a started block without it.
        let mut task = block.task.lock().unwrap_or_else(|e| e.into_inner());
        *task = Some(
            self.handle
                .spawn(crate::turnstile::scope_block(identity, run)),
        );
        Ok(())
    }
}

/// A transcript store opened for one block, awaiting its load.
enum TranscriptSource {
    Record(HostStore),
    Replay(HostStore),
    Seek(HostStore, Option<u64>),
}

impl TranscriptSource {
    /// Load the transcript. A seek's prefix must be reconstructible for the
    /// handoff to be sound: effects into wired peers were suppressed during
    /// replay, and a live world missing the block's own effects is refused
    /// loudly rather than handed a confused block. The profile also says
    /// how far to fast-forward seeded sources.
    async fn open(
        self,
        key: &str,
        sources: &crate::determinism::IsoSources,
    ) -> std::result::Result<BlockTranscript, String> {
        match self {
            TranscriptSource::Record(store) => Ok(BlockTranscript::recording(store)),
            TranscriptSource::Replay(store) => BlockTranscript::replaying(store)
                .await
                .map_err(|e| format!("transcript for block '{key}': {e}")),
            TranscriptSource::Seek(store, to) => {
                let transcript = BlockTranscript::seeking(store, to)
                    .await
                    .map_err(|e| format!("seek transcript for block '{key}': {e}"))?;
                let profile = transcript.preamble_profile();
                if let Some((index, at)) = &profile.peer_write {
                    return Err(format!(
                        "cannot seek block '{key}' past entry {index}: the prefix writes \
                         to '{at}', an effect the live world will not hold — seek \
                         before it, or replay the whole run"
                    ));
                }
                sources.fast_forward(profile.entropy_words, profile.clock_ticks);
                Ok(transcript)
            }
        }
    }
}

/// One block's whole run: wait for a turn (under simulation), load its
/// transcript, build its namespace, drive its code, and record the outcome.
async fn run_block(
    ctx: Arc<RtCtx>,
    block: Arc<BlockRuntime>,
    iso: IsoConfig,
    transcript: Option<TranscriptSource>,
    session: Option<SessionWitness>,
) {
    let turnstile = ctx.turnstile.clone();
    let key = block.transcript_key.clone();
    if let Some(turnstile) = &turnstile {
        turnstile.start(&key).await;
    }
    let mut run_guard = RunGuard(block.clone(), true);
    let result = drive(&ctx, &block, iso, transcript, session).await;
    if let Some(turnstile) = &turnstile {
        turnstile.exit(&key);
    }
    finalize(&block, result);
    run_guard.1 = false;
}

async fn drive(
    ctx: &Arc<RtCtx>,
    block: &Arc<BlockRuntime>,
    iso: IsoConfig,
    transcript: Option<TranscriptSource>,
    session: Option<SessionWitness>,
) -> std::result::Result<(), String> {
    let transcript = match transcript {
        Some(source) => Some(source.open(&block.transcript_key, &iso.sources).await?),
        None => None,
    };
    let mut namespace = Namespace::new(
        ctx.clone(),
        Arc::new(IsoSurface::new(iso)),
        block.wiring.clone(),
        block.provider_owner.clone(),
        block.cell.clone(),
        transcript,
        session,
    );
    // Spec 05 ties Running to "begins reading requests", but an
    // interactive or client-only block may never read them; the
    // strawman marks Running when the driver's code starts.
    block.cell.set_state(BlockState::Running);

    let _watch = ctx.execution_scope().map(|scope| {
        let cell = block.cell.clone();
        crate::execution::Watch(tokio::spawn(async move {
            scope.ended().await;
            cell.request_shutdown(ShutdownMode::Immediate);
        }))
    });
    match &block.driver {
        Driver::Native(factory) => {
            let factory = factory.clone();
            crate::turnstile::blocking(move || {
                let mut native = factory.create();
                native.run(&mut namespace).map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
        }
        Driver::Wasm(driver, format) => driver
            .clone()
            .execute(crate::DriverContext {
                id: block.cell.id.clone(),
                namespace,
                format: format.clone(),
                metering: ctx.metering.clone(),
                cancel: block.cell.cancel.clone(),
                execution: ctx.execution_scope(),
                calls: ctx.call_budget.clone(),
                usage: block.cell.usage.clone(),
            })
            .await
            .map_err(|e| e.to_string())
            .map(|code| {
                // Spec 11: run's return value is the exit code, unless the
                // block already declared one via shutdown/complete.
                if code != 0 && !block.cell.shutdown_complete() {
                    block.cell.mark_shutdown_complete(code as i64);
                }
            }),
    }
}

struct RunGuard(Arc<BlockRuntime>, bool);
impl Drop for RunGuard {
    fn drop(&mut self) {
        if self.1 {
            finalize(&self.0, Err("driver task aborted or panicked".into()));
        }
    }
}

/// Record a finished driver run on the cell and apply the failure policy.
fn finalize(block: &BlockRuntime, result: std::result::Result<(), String>) {
    block.cell.usage.finish();
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

/// What a shutdown left behind.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ShutdownReport {
    /// Registrations whose execution tasks have not been joined. Retain host
    /// reservations until a subsequent shutdown reports an empty list.
    pub remaining: Vec<BlockId>,
    /// Owned provider work and cleanup that may outlive guest execution.
    pub providers: Vec<structfs_service::CloseReport>,
}
impl ShutdownReport {
    pub fn complete(&self) -> bool {
        self.remaining.is_empty() && self.providers.iter().all(|p| p.is_quiescent())
    }
}

/// A running (or runnable) assembly.
#[non_exhaustive]
pub struct AssemblyInstance {
    shutdown_lock: tokio::sync::Mutex<()>,
    provider_owner: structfs_service::Owner,
    /// The assembly's name from its definition.
    pub name: String,
    ctx: Arc<RtCtx>,
    cells: BTreeMap<String, Arc<BlockCell>>,
    public: Arc<BlockCell>,
    children: Vec<Arc<AssemblyInstance>>,
    /// Transcript-key claims made for this subtree, so a failed parent
    /// instantiation can release them.
    claims: Vec<KeyClaim>,
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

/// One client's operations on a persistent assembly. Dropping this owner
/// cancels its pending calls, not the instance or other clients. This does not
/// undo guest effects or attribute the guest's internal CPU to this request.
pub struct AssemblyRequest {
    assembly: Arc<AssemblyInstance>,
    scope: crate::ExecutionScope,
    budget: Arc<crate::CallBudget>,
}
impl AssemblyRequest {
    /// Cancel this client's in-flight calls. Not a stop verb: the request
    /// owns no resources of its own (the instance does), so there is nothing
    /// to `close` or `join`; pending and later calls fail `Cancelled`.
    pub fn cancel(&self) {
        self.scope.cancel();
    }
    pub fn budget(&self) -> &Arc<crate::CallBudget> {
        &self.budget
    }
    pub async fn read(&self, path: Path) -> std::result::Result<Option<Value>, Error> {
        self.scope
            .run(async {
                let _charge =
                    self.budget
                        .acquire(&self.assembly.public.admission_id, &path, &Value::Null)?;
                self.assembly.read(path).await
            })
            .await
    }
    pub async fn write(&self, path: Path, data: Value) -> std::result::Result<Path, Error> {
        self.scope
            .run(async {
                let _charge =
                    self.budget
                        .acquire(&self.assembly.public.admission_id, &path, &data)?;
                self.assembly.write(path, data).await
            })
            .await
    }
}
impl Drop for AssemblyRequest {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl AssemblyInstance {
    /// Start a request scope without changing the persistent instance's scope.
    /// Runtime admission is charged separately, so this local budget has no
    /// parent and cannot replace or bypass the runtime's shared budget.
    pub fn request(
        self: &Arc<Self>,
        timeout: Duration,
        limits: crate::CallLimits,
    ) -> AssemblyRequest {
        AssemblyRequest {
            assembly: self.clone(),
            scope: crate::ExecutionScope::new(timeout),
            budget: crate::CallBudget::shared(limits),
        }
    }

    /// The public block — the assembly's identity from outside.
    pub fn public_cell(&self) -> BlockView {
        BlockView(self.public.clone())
    }

    /// Look up a block by local name.
    pub fn cell(&self, name: &str) -> Option<BlockView> {
        self.cells.get(name).map(|cell| BlockView(cell.clone()))
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

    /// Deliver a signal to a named block's mailbox. False when the block
    /// does not exist, has stopped, or its event budget is full.
    pub fn signal(&self, block: &str, name: impl Into<String>, data: Value) -> bool {
        match self.cells.get(block) {
            Some(cell) => cell.deliver_signal(name, data).is_ok(),
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
        let cell = self.named(name)?;
        self.ctx.call_read(cell, path).await
    }

    /// Host escape hatch: write to a named internal block's store.
    pub async fn write_block(
        &self,
        name: &str,
        path: Path,
        data: Value,
    ) -> std::result::Result<Path, Error> {
        let cell = self.named(name)?;
        self.ctx.call_write(cell, path, data).await
    }

    fn named(&self, name: &str) -> std::result::Result<&Arc<BlockCell>, Error> {
        self.cells
            .get(name)
            .ok_or_else(|| Error::invalid_argument(format!("no block '{name}' in this assembly")))
    }

    fn all_cells(&self) -> Vec<Arc<BlockCell>> {
        let mut cells: Vec<_> = self.cells.values().cloned().collect();
        for child in &self.children {
            cells.extend(child.all_cells());
        }
        cells
    }

    fn provider_owners(&self) -> Vec<structfs_service::OwnerHandle> {
        let mut owners = vec![self.provider_owner.handle()];
        for child in &self.children {
            owners.extend(child.provider_owners());
        }
        owners
    }

    /// Synchronously request graceful shutdown of every block. Parked
    /// mailbox reads unblock immediately; use [`AssemblyInstance::shutdown`]
    /// to also wait and escalate.
    pub fn request_shutdown(&self) {
        for cell in self.all_cells() {
            cell.request_shutdown(ShutdownMode::Graceful);
        }
    }

    /// Shut the assembly down
    /// ([spec 05](https://github.com/StructFS/structfs/blob/main/isotope/spec/05-lifecycle.md))
    /// within one deadline, `timeout` from now, for the whole tree.
    ///
    /// Every block is asked to stop gracefully. Blocks still running at
    /// the halfway point are escalated to immediate shutdown (their parked
    /// reads fail and running guests are interrupted). Driver tasks and
    /// provider cleanup are then joined until the deadline. Whatever has
    /// not been joined by then is reported, not waited for: retain host
    /// reservations until a later `shutdown` reports [`ShutdownReport::complete`].
    pub async fn shutdown(&self, timeout: Duration) -> ShutdownReport {
        let _shutdown = self.shutdown_lock.lock().await;
        let start = tokio::time::Instant::now();
        let deadline = start + timeout;
        let escalate_at = start + timeout / 2;
        let cells = self.all_cells();
        for cell in &cells {
            cell.request_shutdown(ShutdownMode::Graceful);
        }
        for cell in &cells {
            let _ = tokio::time::timeout_at(escalate_at, cell.wait_terminal()).await;
        }
        for cell in &cells {
            if !cell.state().is_terminal() {
                cell.request_shutdown(ShutdownMode::Immediate);
            }
        }
        let owners = self.provider_owners();
        for owner in &owners {
            owner.close();
        }
        for cell in &cells {
            let block = self.ctx.lock_blocks().get(&cell.id).cloned();
            let Some(block) = block else { continue };
            let task = block.task.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(mut task) = task {
                if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                    // A native driver may not cooperate; retain its handle
                    // and registration rather than claim it has stopped.
                    *block.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
                    continue;
                }
            }
            if cell.state().is_terminal() {
                self.ctx.lock_blocks().remove(&cell.id);
            }
        }
        let mut providers = Vec::new();
        for owner in owners {
            providers.push(
                owner
                    .join(deadline.saturating_duration_since(tokio::time::Instant::now()))
                    .await,
            );
        }
        let blocks = self.ctx.lock_blocks();
        ShutdownReport {
            providers,
            remaining: cells
                .iter()
                .filter(|cell| blocks.contains_key(&cell.id))
                .map(|cell| cell.id.clone())
                .collect(),
        }
    }
}

/// The shared core of a [`Runtime`], referenced by spawn/management
/// stores so blocks can instantiate assemblies through the store surface.
pub(crate) struct RuntimeInner {
    ctx: Arc<RtCtx>,
    builtins: HashMap<String, Arc<dyn NativeBlockFactory>>,
    loaders: Vec<Arc<dyn ArtifactLoader>>,
    prepared: HashMap<String, Arc<dyn WasmBlockDriver>>,
}

/// Undoes a partial instantiation: every block registered (or started) in
/// this scope and its nested children is shut down and deregistered unless
/// the instantiation completes.
///
/// Driver tasks that already started are not abandoned: their handles are
/// joined on a task supervised by the runtime's cleanup supervisor, so they
/// stay visible (and a failed join is counted) in
/// `Runtime::cleanup_supervisor().reports()` until they end. Transcript-key
/// claims are released too, so a retried instantiation gets the same keys —
/// and therefore the same block identities — a first success would have.
struct Rollback {
    ctx: Arc<RtCtx>,
    cells: Vec<Arc<BlockCell>>,
    children: std::cell::RefCell<Vec<Arc<AssemblyInstance>>>,
    claims: std::cell::RefCell<Vec<KeyClaim>>,
    armed: bool,
}

/// One claim on a transcript key base: the `n`th use of `base`.
#[derive(Clone)]
struct KeyClaim {
    base: String,
    n: u64,
}

impl Drop for Rollback {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut cells = self.cells.clone();
        let mut claims = self.claims.get_mut().clone();
        for child in self.children.get_mut().iter() {
            cells.extend(child.all_cells());
            claims.extend(child.claims.iter().cloned());
        }
        for cell in &cells {
            cell.request_shutdown(ShutdownMode::Immediate);
        }
        let started: Vec<tokio::task::JoinHandle<()>> = {
            let mut blocks = self.ctx.lock_blocks();
            cells
                .iter()
                .filter_map(|cell| blocks.remove(&cell.id))
                .filter_map(|block| block.task.lock().unwrap_or_else(|e| e.into_inner()).take())
                .collect()
        };
        {
            // Release a claim only while it is still the latest use of its
            // base: a concurrent instantiation may have claimed after us.
            let mut keys = self
                .ctx
                .transcript_keys
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for claim in claims.iter().rev() {
                if keys.get(&claim.base) == Some(&claim.n) {
                    if claim.n <= 1 {
                        keys.remove(&claim.base);
                    } else {
                        keys.insert(claim.base.clone(), claim.n - 1);
                    }
                }
            }
        }
        if started.is_empty() {
            return;
        }
        let join = |_cancel| async move {
            for task in started {
                task.await
                    .map_err(|e| Error::store("runtime", "rollback", e.to_string()))?;
            }
            Ok(())
        };
        match self
            .ctx
            .cleanup
            .owner(structfs_service::OwnerLimits::default().with_resources(1))
        {
            Ok(owner) => {
                if let Err(error) = owner.handle().spawn(join) {
                    tracing::warn!(%error, "could not supervise a failed instantiation's tasks");
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cleanup supervisor full; joining unsupervised");
                self.ctx.handle.spawn(async move {
                    let _ = join(structfs_handles::CancelToken::new()).await;
                });
            }
        }
    }
}

impl RuntimeInner {
    /// The shared runtime context (for grant stores and spawn surfaces).
    pub(crate) fn ctx(&self) -> Arc<RtCtx> {
        self.ctx.clone()
    }

    /// Load a wasm artifact through the registered binding loaders.
    fn load_artifact(&self, artifact: &str, bytes: Vec<u8>) -> Result<Arc<dyn WasmBlockDriver>> {
        // Loaders may create engines and ticker tasks: enter the runtime so
        // synchronous callers outside it can instantiate.
        let _entered = self.ctx.handle.enter();
        match self.loaders.iter().find(|loader| loader.matches(&bytes)) {
            Some(loader) => loader.load(bytes),
            None => Err(RuntimeError::assembly(format!(
                "no registered artifact loader recognizes '{artifact}' \
                 (adapters add bindings via RuntimeConfig::register_loader)"
            ))),
        }
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
        let instance = self.instantiate_scoped(def, imports, base_dir, &def.name);
        // Simulation: the first scheduling decision waits until the
        // WHOLE tree — nested assemblies included — is enrolled, so the
        // schedule never races the host thread's remaining enrollment.
        // (A spawner instantiating mid-run holds the turn, so this is a
        // no-op there and the children start at its next yield.) A failed
        // instantiation launches too, so blocks it already enrolled get
        // the turns they need to observe their shutdown.
        if let Some(turnstile) = self.ctx.turnstile() {
            turnstile.launch();
        }
        instance
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

        let provider_owner = self
            .ctx
            .cleanup
            .owner(structfs_service::OwnerLimits::default().with_resources(PROVIDER_RESOURCES))
            .map_err(|e| RuntimeError::Admission(e.to_string()))?;
        let mut rollback = Rollback {
            ctx: self.ctx.clone(),
            cells: Vec::new(),
            children: std::cell::RefCell::new(Vec::new()),
            claims: std::cell::RefCell::new(Vec::new()),
            armed: true,
        };
        let mut cells: BTreeMap<String, Arc<BlockCell>> = BTreeMap::new();
        let mut drivers: BTreeMap<String, Driver> = BTreeMap::new();
        let mut keys: BTreeMap<String, String> = BTreeMap::new();

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
            let n = *uses;
            rollback.claims.borrow_mut().push(KeyClaim {
                base: base.clone(),
                n,
            });
            match n {
                1 => base,
                n => format!("{base}#{n}"),
            }
        };
        let mut new_cell = |name: &str, driver: Driver| {
            let key = claim_key(name);
            let mut cell = BlockCell::keyed(name.to_string(), def.failure_policy(name), &key);
            if let Some(turnstile) = self.ctx.turnstile() {
                cell.attach_simulation(&key, turnstile.clone());
            }
            cells.insert(name.to_string(), Arc::new(cell));
            keys.insert(name.to_string(), key);
            drivers.insert(name.to_string(), driver);
        };

        // Create cells (or recurse for nested assemblies).
        let mut children = Vec::new();
        for (name, block_def) in &def.blocks {
            let artifact = block_def.artifact.as_str();
            let prepared = self.prepared.get(artifact).cloned();
            if let Some(builtin) = artifact.strip_prefix("builtin:") {
                let factory = self.builtins.get(builtin).cloned().ok_or_else(|| {
                    RuntimeError::assembly(format!("unknown builtin block '{builtin}'"))
                })?;
                new_cell(name, Driver::Native(factory));
            } else if prepared.is_some() || artifact.ends_with(".wasm") {
                let driver = match prepared {
                    Some(driver) => driver,
                    None => {
                        self.load_artifact(artifact, std::fs::read(base_dir.join(artifact))?)?
                    }
                };
                let format = wasm_format(&driver.manifest()?, block_def.serialization.as_str())?;
                new_cell(name, Driver::Wasm(driver, format));
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
                rollback.children.borrow_mut().push(child.clone());
                children.push((name.clone(), child));
            } else {
                return Err(RuntimeError::assembly(format!(
                    "unsupported artifact reference '{artifact}' \
                     (expected builtin:{{name}}, *.wasm, or a nested *.json/*.yaml)"
                )));
            }
        }
        for (name, child) in &children {
            cells.insert(name.clone(), child.public.clone());
        }
        let children: Vec<_> = children.into_iter().map(|(_, child)| child).collect();

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
                entries.push((path!("config"), Target::Store(host_store(store))));
            }
            for wire in def.wiring.iter().filter(|w| w.block == name) {
                let target = match &wire.target {
                    WireTarget::Block(target_name) => Target::Block(cells[target_name].clone()),
                    WireTarget::Import(import) => Target::Store(imports[import].clone()),
                };
                entries.push((wire.prefix.clone(), target));
            }

            rollback.cells.push(cell.clone());
            self.ctx.lock_blocks().insert(
                cell.id.clone(),
                Arc::new(BlockRuntime {
                    cell,
                    driver,
                    wiring: Arc::new(WiringTable::new(entries)),
                    provider_owner: provider_owner.handle(),
                    siblings: siblings.clone(),
                    env: Arc::new(block_def.env.clone()),
                    args: Arc::new(block_def.args.clone()),
                    stdio_kind: block_def.stdio.clone(),
                    spawn: block_def.spawn,
                    base_dir: base_dir.to_path_buf(),
                    transcript_key: keys[&name].clone(),
                    task: Mutex::new(None),
                }),
            );
        }

        // The public block starts eagerly; everything else is lazy —
        // except under simulation, where every block starts at once:
        // autonomous blocks are what concurrency means, and the seeded
        // schedule needs them all in the runnable set from the top.
        self.ctx.ensure_started(&public)?;
        if self.ctx.turnstile().is_some() {
            // Under simulation every block starts (and enrolls) here,
            // in deterministic iteration order; the launch happens once,
            // at the top of the tree, after all enrollment.
            for cell in cells.values() {
                self.ctx.ensure_started(cell)?;
            }
        }
        rollback.armed = false;
        let mut claims = rollback.claims.take();
        for child in &children {
            claims.extend(child.claims.iter().cloned());
        }
        Ok(Arc::new(AssemblyInstance {
            shutdown_lock: tokio::sync::Mutex::new(()),
            provider_owner,
            name: def.name.clone(),
            ctx: self.ctx.clone(),
            cells,
            public,
            children,
            claims,
        }))
    }
}

/// The Featherweight runtime.
///
/// Holds the frozen [`RuntimeConfig`], the block registry, and the shared
/// context. Blocks run as async tasks on the configured Tokio runtime;
/// native blocks use its blocking pool.
pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

impl Runtime {
    /// Freeze `config` into a runtime.
    pub fn new(config: RuntimeConfig) -> Self {
        let RuntimeConfig {
            handle,
            timeout,
            call_budget,
            execution,
            log,
            stdio_provider,
            metering,
            transcripts,
            determinism,
            session,
            core_engine,
            builtins,
            mut loaders,
            artifacts,
        } = config;
        // The built-in core-wasm loader is consulted last.
        let core = CoreWasmLoader {
            engine: std::sync::OnceLock::new(),
        };
        if let Some(engine) = core_engine {
            let _ = core.engine.set(engine);
        }
        loaders.push(Arc::new(core));
        let inner = Arc::new_cyclic(|weak: &Weak<RuntimeInner>| {
            let turnstile = determinism.simulation_seed().map(|seed| {
                let turnstile = Turnstile::new(seed);
                let runtime = weak.clone();
                // A wedged schedule can never recover on its own (all wakes
                // happen on turns), so shut the assembly down loudly — in
                // registry order, which under simulation is key order.
                turnstile.set_deadlock_handler(move || {
                    if let Some(runtime) = runtime.upgrade() {
                        let blocks: Vec<_> = runtime.ctx.lock_blocks().values().cloned().collect();
                        for block in blocks {
                            block.cell.request_shutdown(ShutdownMode::Immediate);
                            // Free any caller stuck awaiting this block: the
                            // cycle means its response is never coming.
                            block.cell.fail_in_flight();
                        }
                    }
                });
                turnstile
            });
            RuntimeInner {
                ctx: Arc::new(RtCtx {
                    cleanup: Arc::new(structfs_service::CleanupSupervisor::with_handle(
                        PROVIDER_RESOURCES,
                        handle.clone(),
                    )),
                    handle,
                    timeout,
                    call_budget,
                    execution,
                    log,
                    stdio_provider,
                    metering,
                    transcript_mode: transcripts,
                    determinism,
                    session: session.map(SessionLog::new),
                    turnstile,
                    blocks: Mutex::new(BTreeMap::new()),
                    transcript_keys: Mutex::new(HashMap::new()),
                    runtime: weak.clone(),
                }),
                builtins,
                loaders,
                prepared: artifacts,
            }
        });
        Self { inner }
    }

    /// Retain this supervisor through host shutdown to observe provider cleanup
    /// (and the shutdown of released spawn handles) even when an instance or its
    /// shutdown future is dropped.
    pub fn cleanup_supervisor(&self) -> Arc<structfs_service::CleanupSupervisor> {
        self.inner.ctx.cleanup.clone()
    }

    /// Number of retained block registrations, including lazy blocks.
    pub fn registered_blocks(&self) -> usize {
        self.inner.ctx.lock_blocks().len()
    }

    /// Instantiate an assembly definition.
    ///
    /// `imports` provides stores for the definition's declared imports;
    /// `base_dir` resolves relative artifact paths (wasm files, nested
    /// definitions). On error nothing stays registered: blocks already
    /// created (nested assemblies included) are shut down and deregistered.
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
        "application/vnd.structfs.value+json;version=1" => Ok(Format::VALUE_JSON),
        other => Err(RuntimeError::Manifest(format!(
            "unsupported serialization '{other}' (supported: application/json, \
             application/cbor, application/x-flexbuffers, application/vnd.structfs.value+json;version=1)"
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
            (
                "application/vnd.structfs.value+json;version=1",
                Format::VALUE_JSON,
            ),
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
