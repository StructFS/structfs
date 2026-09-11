//! Per-block namespaces
//! ([spec 03](https://github.com/StructFS/structfs/blob/main/isotope/spec/03-namespaces.md)).
//!
//! A block's namespace is its complete view of the world: `iso/` is the
//! runtime, everything else is wired by the assembly. Paths are rewritten
//! component-wise at mount boundaries in both directions — targets see
//! paths relative to their root, and write result paths come back
//! expressed in the caller's namespace.

use std::sync::Arc;

use structfs_core_store::{Error, Path, Reader, Record, Shared, Store, Value, Writer};

use crate::block::BlockCell;
use crate::iso::IsoSurface;
use crate::runtime::RtCtx;
use crate::session::SessionLog;
use crate::transcript::BlockTranscript;

/// One block's line into the session log: the log plus the identity
/// entries are witnessed under.
pub(crate) struct SessionWitness {
    pub(crate) log: Arc<SessionLog>,
    pub(crate) block: String,
}

/// A shared host-side store (config, imports).
#[derive(Clone)]
pub enum HostStore {
    /// Synchronous providers execute on the blocking pool per operation.
    Sync(Shared<Box<dyn Store>>),
    /// Detached providers release their lock before awaiting I/O.
    Async(Arc<std::sync::Mutex<Box<dyn structfs_core_store::DetachedStore>>>),
    /// Delegation routes asynchronously, including grants of grants.
    Grant(Arc<GrantStore>),
}

impl HostStore {
    fn read_async(
        &self,
        from: &Path,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Record>, Error>> + Send + '_>,
    > {
        let from = from.clone();
        Box::pin(async move {
            match self {
                Self::Sync(store) => {
                    let mut store = store.clone();
                    crate::turnstile::blocking(move || store.read(&from))
                        .await
                        .map_err(|e| Error::store("host", "read", e.to_string()))?
                }
                Self::Async(store) => {
                    let future = store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .read_detached(&from);
                    future.await
                }
                Self::Grant(grant) => {
                    let rel = grant.base.join(&from);
                    match &grant.target {
                        Target::Block(cell) => grant
                            .ctx
                            .call_read(cell, rel)
                            .await
                            .map(|v| v.map(Record::parsed)),
                        Target::Store(store) => store.read_async(&rel).await,
                    }
                }
            }
        })
    }

    fn write_async(
        &self,
        to: &Path,
        data: Record,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Path, Error>> + Send + '_>> {
        let to = to.clone();
        Box::pin(async move {
            match self {
                Self::Sync(store) => {
                    let mut store = store.clone();
                    crate::turnstile::blocking(move || store.write(&to, data))
                        .await
                        .map_err(|e| Error::store("host", "write", e.to_string()))?
                }
                Self::Async(store) => {
                    let future = store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .write_detached(&to, data);
                    future.await
                }
                Self::Grant(grant) => {
                    let rel = grant.base.join(&to);
                    let result = match &grant.target {
                        Target::Block(cell) => {
                            grant
                                .ctx
                                .call_write(
                                    cell,
                                    rel,
                                    data.into_value(&structfs_core_store::NoCodec)?,
                                )
                                .await?
                        }
                        Target::Store(store) => store.write_async(&rel, data).await?,
                    };
                    result.strip_prefix(&grant.base).ok_or_else(|| {
                        Error::store("grant", "write", "target returned path outside the grant")
                    })
                }
            }
        })
    }
}

impl Reader for HostStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match self {
            Self::Sync(store) => store.read(from),
            _ => tokio::runtime::Handle::current().block_on(self.read_async(from)),
        }
    }
}

impl Writer for HostStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        match self {
            Self::Sync(store) => store.write(to, data),
            _ => tokio::runtime::Handle::current().block_on(self.write_async(to, data)),
        }
    }
}

/// Mount a concurrent async provider without holding its mutex across I/O.
/// Detached methods must return promptly; their futures do the waiting.
pub fn async_host_store(store: impl structfs_core_store::DetachedStore + 'static) -> HostStore {
    HostStore::Async(Arc::new(std::sync::Mutex::new(Box::new(store))))
}

/// Wrap any store as a [`HostStore`].
pub fn host_store(store: impl Store + 'static) -> HostStore {
    HostStore::Sync(Shared::new(Box::new(store) as Box<dyn Store>))
}

/// A wiring target: another block (via the server protocol) or a
/// host-side store.
#[derive(Clone)]
pub enum Target {
    /// Operations become server-protocol requests to this block.
    Block(Arc<BlockCell>),
    /// Operations go directly to a host store.
    Store(HostStore),
}

/// Longest-prefix, component-wise wiring table.
pub struct WiringTable {
    /// Entries sorted longest-prefix-first, so the first component-wise
    /// match wins (mount shadowing per spec 03).
    entries: Vec<(Path, Target)>,
}

impl WiringTable {
    /// Build a table; entries are sorted longest-prefix-first.
    pub fn new(mut entries: Vec<(Path, Target)>) -> Self {
        entries.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        Self { entries }
    }

    /// Resolve a path to `(target, relative path, mount prefix)`.
    /// Component-wise: `services/cache` does not match `services/cache_x`.
    pub fn resolve<'t>(&'t self, path: &Path) -> Option<(&'t Target, Path, &'t Path)> {
        for (prefix, target) in &self.entries {
            if let Some(rel) = path.strip_prefix(prefix) {
                return Some((target, rel, prefix));
            }
        }
        None
    }

    /// The wired mount prefixes (for namespace listings).
    pub fn prefixes(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().map(|(p, _)| p)
    }
}

/// A slice of a namespace, packaged as a host store.
///
/// Operations join `base` and route to the captured target. This is how
/// spawn-time grants hand a child an attenuation of the spawner's
/// capabilities: the child mounts this store wherever its own definition
/// says, and can never see outside `base`.
pub struct GrantStore {
    ctx: Arc<RtCtx>,
    target: Target,
    base: Path,
}

impl GrantStore {
    pub(crate) fn new(ctx: Arc<RtCtx>, target: Target, base: Path) -> Self {
        Self { ctx, target, base }
    }
}

impl Reader for GrantStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let rel = self.base.join(from);
        match &self.target {
            Target::Block(cell) => {
                let cell = cell.clone();
                self.ctx
                    .block_on(self.ctx.call_read(&cell, rel))
                    .map(|v| v.map(Record::parsed))
            }
            Target::Store(store) => store.clone().read(&rel),
        }
    }
}

impl Writer for GrantStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let rel = self.base.join(to);
        let result = match &self.target {
            Target::Block(cell) => {
                let cell = cell.clone();
                let value = data.into_value(&structfs_core_store::NoCodec)?;
                self.ctx.block_on(self.ctx.call_write(&cell, rel, value))?
            }
            Target::Store(store) => store.clone().write(&rel, data)?,
        };
        // Result paths are expressed relative to the grant, never
        // revealing the base (the confinement rule Rooted also follows).
        result.strip_prefix(&self.base).ok_or_else(|| {
            Error::store(
                "grant",
                "write",
                format!("target returned path outside the grant: {}", result),
            )
        })
    }
}

/// A block's namespace. Async operations suspend without owning a thread.
/// The synchronous Reader/Writer facade is for native blocking drivers only.
pub struct Namespace {
    ctx: Arc<RtCtx>,
    iso: Arc<IsoSurface>,
    wiring: Arc<WiringTable>,
    cell: Arc<BlockCell>,
    /// Transcripts (spec 12): every operation through this namespace is
    /// recorded to, or answered from, the block's transcript.
    transcript: Option<BlockTranscript>,
    /// The session log (spec 12): a forensic witness of every
    /// operation's arrival order across the assembly. Observation-class:
    /// it answers nothing and never fails an operation.
    session: Option<SessionWitness>,
}

impl Namespace {
    pub(crate) fn new(
        ctx: Arc<RtCtx>,
        iso: Arc<IsoSurface>,
        wiring: Arc<WiringTable>,
        cell: Arc<BlockCell>,
        transcript: Option<BlockTranscript>,
        session: Option<SessionWitness>,
    ) -> Self {
        Self {
            ctx,
            iso,
            wiring,
            cell,
            transcript,
            session,
        }
    }

    /// The owning block's cell (id, state, shutdown flags).
    pub fn cell(&self) -> &Arc<BlockCell> {
        &self.cell
    }

    /// Native/adapter cancellation token for a request served by this block.
    /// Use it to cancel the request's provider waits without stopping the block.
    pub fn request_cancellation(
        &self,
        response: &Path,
    ) -> Result<structfs_handles::CancelToken, Error> {
        let parts: Vec<_> = response.iter().collect();
        match parts.as_slice() {
            ["iso", "server", "responses", token] => {
                let token = token
                    .parse()
                    .map_err(|_| Error::conflict("invalid response token"))?;
                Ok(self.cell.request_cancellation(token))
            }
            _ => Err(Error::permission_denied(
                "not a response path in this block",
            )),
        }
    }

    fn root_listing(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert("iso".to_string(), Value::from("Isotope system services"));
        for prefix in self.wiring.prefixes() {
            if !prefix.is_empty() {
                map.insert(prefix[0].to_string(), Value::from("wired"));
            }
        }
        Value::Map(map)
    }
}

impl Namespace {
    async fn read_live(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(self.root_listing())));
        }
        if from == &structfs_core_store::path!("iso/capabilities") {
            return Ok(Some(Record::parsed(Value::Array(
                self.wiring
                    .prefixes()
                    .map(|path| Value::String(path.to_string()))
                    .collect(),
            ))));
        }
        if from == &structfs_core_store::path!("iso/execution/budget") {
            let view = serde_json::json!({
                "scope": "instance",
                "calls": self.ctx.call_budget_snapshot(),
                "events": self.cell.events.snapshot(),
                "replies": self.cell.replies.snapshot(),
                "execution": self.cell.usage.snapshot(),
                "deadline_remaining_ms": self.ctx.execution_scope().map(|scope|
                    scope.deadline().saturating_duration_since(tokio::time::Instant::now()).as_millis()),
                "units": { "calls_bytes": "logical_payload_bytes", "linear_memory_bytes": "wasm_linear_memory_bytes",
                    "wasm_fuel_consumed": "wasmtime_fuel" },
            });
            return Ok(Some(Record::parsed(structfs_serde_store::json_to_value(
                view,
            ))));
        }
        if &from[0] == "iso" {
            let rel = from.slice(1, from.len());
            return self.iso.read(&rel).await;
        }
        match self.wiring.resolve(from) {
            Some((Target::Block(cell), rel, _prefix)) => {
                let cell = cell.clone();
                self.ctx
                    .call_read(&cell, rel)
                    .await
                    .map(|v| v.map(Record::parsed))
            }
            Some((Target::Store(store), rel, _prefix)) => store.read_async(&rel).await,
            // Unwired paths are denied (spec 03): a capability system
            // must not leak absence vs denial.
            None => Err(Error::permission_denied(format!(
                "path is not wired into this namespace: {}",
                from
            ))),
        }
    }

    async fn write_live(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::permission_denied("namespace root is not writable"));
        }
        if &to[0] == "iso" {
            let rel = to.slice(1, to.len());
            let value = data.into_value(&structfs_core_store::NoCodec)?;
            let result = self.iso.write(&rel, value).await?;
            return Ok(Path::parse("iso").unwrap().join(&result));
        }
        match self.wiring.resolve(to) {
            Some((Target::Block(cell), rel, prefix)) => {
                let cell = cell.clone();
                let value = data.into_value(&structfs_core_store::NoCodec)?;
                let result = self.ctx.call_write(&cell, rel, value).await?;
                // Result paths are expressed in the caller's namespace.
                Ok(prefix.join(&result))
            }
            Some((Target::Store(store), rel, prefix)) => {
                let result = store.write_async(&rel, data).await?;
                Ok(prefix.join(&result))
            }
            // Unwired writes are a capability failure (spec 03: "write → error").
            None => Err(Error::permission_denied(format!(
                "path is not wired into this namespace: {}",
                to
            ))),
        }
    }
}

// The transcript interposes at the trait impls so that every operation a block
// makes — iso, wired services, even the root listing — crosses it
// uniformly (spec 12). Under replay the live world is never consulted:
// no iso surface, no wiring targets, no effects.
impl Namespace {
    /// Witness one completed operation in the session log, if one is
    /// attached. `entry` is the transcript index the operation occupied
    /// or consumed; forensics never fails the operation it observes.
    fn witness(&self, op: &str, at: &Path, outcome: String, entry: Option<u64>) {
        if let Some(session) = &self.session {
            session.log.witness(&session.block, op, at, outcome, entry);
        }
    }
}

impl Namespace {
    /// Under simulation, every boundary operation is a seeded
    /// interleaving point: yield the turn and let the schedule decide
    /// who runs next. A no-op outside simulation — the performance path
    /// pays one None check.
    async fn sim_yield(&self) {
        if let Some((key, turnstile)) = self.cell.sim() {
            let turnstile = turnstile.clone();
            let key = key.clone();
            turnstile.yield_now(&key).await;
        }
    }

    /// A seek whose replay reached its horizon hands off here: the
    /// transcript goes inert and every subsequent operation runs live.
    fn hand_off_if_ready(&mut self) {
        if self
            .transcript
            .as_ref()
            .is_some_and(BlockTranscript::handoff_ready)
        {
            self.transcript = Some(BlockTranscript::HandedOff);
            tracing::info!(block = %self.cell.name, "seek reached its horizon; continuing live");
        }
    }
}

#[async_trait::async_trait]
impl structfs_core_store::AsyncReader for Namespace {
    async fn read_async(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.hand_off_if_ready();
        let entry = self.transcript.as_ref().and_then(BlockTranscript::position);
        match &mut self.transcript {
            Some(transcript) if transcript.is_replaying() => {
                let result = transcript.replay_read(from);
                self.witness("read", from, crate::session::read_outcome(&result), entry);
                return result;
            }
            _ => {}
        }
        let result = match self.ctx.execution_scope() {
            Some(scope) => scope.run(self.read_live(from)).await,
            None => self.read_live(from).await,
        };
        if let Some(transcript) = &mut self.transcript {
            transcript.record_read(from, &result)?;
        }
        self.witness("read", from, crate::session::read_outcome(&result), entry);
        self.sim_yield().await;
        result
    }
}

#[async_trait::async_trait]
impl structfs_core_store::AsyncWriter for Namespace {
    async fn write_async(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // The digest is computed before dispatch (write_live consumes the
        // record) and only when a transcript will use it.
        let wrote = self
            .transcript
            .as_ref()
            .and_then(|_| crate::transcript::digest(&data));
        self.hand_off_if_ready();
        let entry = self.transcript.as_ref().and_then(BlockTranscript::position);
        match &mut self.transcript {
            Some(transcript) if transcript.is_replaying() => {
                let result = transcript.replay_write(to, wrote);
                self.witness("write", to, crate::session::write_outcome(&result), entry);
                return result;
            }
            _ => {}
        }
        let result = match self.ctx.execution_scope() {
            Some(scope) => scope.run(self.write_live(to, data)).await,
            None => self.write_live(to, data).await,
        };
        if let Some(transcript) = &mut self.transcript {
            transcript.record_write(to, wrote, &result)?;
        }
        self.witness("write", to, crate::session::write_outcome(&result), entry);
        self.sim_yield().await;
        result
    }
}

impl Reader for Namespace {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let ctx = self.ctx.clone();
        ctx.block_on(structfs_core_store::AsyncReader::read_async(self, from))
    }
}

impl Writer for Namespace {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let ctx = self.ctx.clone();
        ctx.block_on(structfs_core_store::AsyncWriter::write_async(
            self, to, data,
        ))
    }
}

impl Drop for Namespace {
    /// A replayed block that exits with entries unconsumed stopped short
    /// of the recorded run. Not an error — a block may legitimately exit
    /// early on a replayed shutdown request — but worth a trace.
    fn drop(&mut self) {
        if let Some(transcript) = &self.transcript {
            let remaining = transcript.remaining();
            if remaining > 0 {
                tracing::warn!(
                    block = %self.cell.name,
                    remaining,
                    "replay ended with transcript entries unconsumed"
                );
            }
        }
    }
}

#[cfg(test)]
mod async_provider_tests {
    use super::*;
    use structfs_core_store::{path, DetachedFuture, DetachedReader, DetachedWriter};

    struct WaitingProvider {
        entered: Arc<tokio::sync::Notify>,
        released: Arc<tokio::sync::Notify>,
    }

    impl DetachedReader for WaitingProvider {
        fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
            let entered = self.entered.clone();
            let released = self.released.clone();
            Box::pin(async move {
                entered.notify_one();
                released.notified().await;
                Ok(Some(Record::parsed(Value::from("released"))))
            })
        }
    }

    impl DetachedWriter for WaitingProvider {
        fn write_detached(&mut self, to: &Path, _: Record) -> DetachedFuture<Path> {
            let to = to.clone();
            let released = self.released.clone();
            Box::pin(async move {
                released.notify_one();
                Ok(to)
            })
        }
    }

    #[tokio::test]
    async fn parked_provider_read_does_not_lock_out_the_releasing_write() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let store = async_host_store(WaitingProvider {
            entered: entered.clone(),
            released: Arc::new(tokio::sync::Notify::new()),
        });
        let reader = store.clone();
        let read = tokio::spawn(async move { reader.read_async(&path!("waiting")).await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            entered.notified().await;
            let written = store
                .write_async(&path!("release"), Record::parsed(Value::Null))
                .await
                .unwrap();
            assert_eq!(written, path!("release"));
            let answer = read.await.unwrap().unwrap().unwrap();
            assert_eq!(answer.as_value(), Some(&Value::from("released")));
        })
        .await
        .expect("provider lock was held across an await");
    }
}
