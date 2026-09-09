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
pub type HostStore = Shared<Box<dyn Store>>;

/// Wrap any store as a [`HostStore`].
pub fn host_store(store: impl Store + 'static) -> HostStore {
    Shared::new(Box::new(store) as Box<dyn Store>)
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

/// A block's namespace, as a synchronous store.
///
/// This is what a native block's `run` receives and what a wasm block's
/// host bridge wraps. Operations that park (server-protocol reads, routed
/// calls) block the calling thread via the runtime handle, so a
/// `Namespace` must only be used from a blocking thread — which is where
/// block code runs.
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
    fn read_live(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(self.root_listing())));
        }
        if &from[0] == "iso" {
            let rel = from.slice(1, from.len());
            return self.ctx.block_on(self.iso.read(&rel));
        }
        match self.wiring.resolve(from) {
            Some((Target::Block(cell), rel, _prefix)) => {
                let cell = cell.clone();
                self.ctx
                    .block_on(self.ctx.call_read(&cell, rel))
                    .map(|v| v.map(Record::parsed))
            }
            Some((Target::Store(store), rel, _prefix)) => store.clone().read(&rel),
            // Unwired paths are denied (spec 03): a capability system
            // must not leak absence vs denial.
            None => Err(Error::permission_denied(format!(
                "path is not wired into this namespace: {}",
                from
            ))),
        }
    }

    fn write_live(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::permission_denied("namespace root is not writable"));
        }
        if &to[0] == "iso" {
            let rel = to.slice(1, to.len());
            let value = data.into_value(&structfs_core_store::NoCodec)?;
            let result = self.ctx.block_on(self.iso.write(&rel, value))?;
            return Ok(Path::parse("iso").unwrap().join(&result));
        }
        match self.wiring.resolve(to) {
            Some((Target::Block(cell), rel, prefix)) => {
                let cell = cell.clone();
                let value = data.into_value(&structfs_core_store::NoCodec)?;
                let result = self.ctx.block_on(self.ctx.call_write(&cell, rel, value))?;
                // Result paths are expressed in the caller's namespace.
                Ok(prefix.join(&result))
            }
            Some((Target::Store(store), rel, prefix)) => {
                let result = store.clone().write(&rel, data)?;
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
    fn sim_yield(&self) {
        if let Some((key, turnstile)) = self.cell.sim() {
            let turnstile = turnstile.clone();
            let key = key.clone();
            self.ctx.block_on(turnstile.yield_now(&key));
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

impl Reader for Namespace {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
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
        let result = self.read_live(from);
        if let Some(transcript) = &mut self.transcript {
            transcript.record_read(from, &result)?;
        }
        self.witness("read", from, crate::session::read_outcome(&result), entry);
        self.sim_yield();
        result
    }
}

impl Writer for Namespace {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
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
        let result = self.write_live(to, data);
        if let Some(transcript) = &mut self.transcript {
            transcript.record_write(to, wrote, &result)?;
        }
        self.witness("write", to, crate::session::write_outcome(&result), entry);
        self.sim_yield();
        result
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
