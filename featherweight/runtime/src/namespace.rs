//! Per-block namespaces (`isotope/spec/03-namespaces.md`).
//!
//! A block's namespace is its complete view of the world: `iso/` is the
//! runtime, everything else is wired by the assembly. Paths are rewritten
//! component-wise at mount boundaries in both directions — targets see
//! paths relative to their root, and write result paths come back
//! expressed in the caller's namespace.

use std::sync::Arc;

use structfs_core_store::{
    Error, Path, Reader, Record, Shared, Store, Value, Writer,
};

use crate::block::BlockCell;
use crate::iso::IsoSurface;
use crate::runtime::RtCtx;

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
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
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
}

impl Namespace {
    pub(crate) fn new(
        ctx: Arc<RtCtx>,
        iso: Arc<IsoSurface>,
        wiring: Arc<WiringTable>,
        cell: Arc<BlockCell>,
    ) -> Self {
        Self {
            ctx,
            iso,
            wiring,
            cell,
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
                map.insert(prefix[0].clone(), Value::from("wired"));
            }
        }
        Value::Map(map)
    }
}

impl Reader for Namespace {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(self.root_listing())));
        }
        if from[0] == "iso" {
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
            // Unwired reads are absent (spec 03: "read → null").
            None => Ok(None),
        }
    }
}

impl Writer for Namespace {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::permission_denied("namespace root is not writable"));
        }
        if to[0] == "iso" {
            let rel = to.slice(1, to.len());
            let value = data.into_value(&structfs_core_store::NoCodec)?;
            let result = self.iso.write(&rel, value)?;
            return Ok(Path::parse("iso").unwrap().join(&result));
        }
        match self.wiring.resolve(to) {
            Some((Target::Block(cell), rel, prefix)) => {
                let cell = cell.clone();
                let value = data.into_value(&structfs_core_store::NoCodec)?;
                let result = self
                    .ctx
                    .block_on(self.ctx.call_write(&cell, rel, value))?;
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
