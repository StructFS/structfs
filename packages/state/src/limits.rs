//! State limits, canonical sizing, and tree validation.
//!
//! Everything here is pure: it takes a value and some limits and answers a
//! question. Nothing in this module touches the provider's mutex, which is
//! the point — canonical encoding is the most expensive thing the provider
//! does, and it must never happen while the single state lock is held more
//! often than once per commit.

use std::time::Duration;

use serde::Serialize;
use structfs_core_store::{Codec, Value};
use structfs_serde_store::{to_value, CodecProfile, ValueCodec};

use crate::Fault;

/// Fixed allowance for the reply envelope around a page's items.
///
/// Page assembly adds up the encoded size of each item it appends rather than
/// re-encoding the whole page every time; this covers the wrapper those items
/// sit inside. [`StateLimits::new`] refuses page budgets smaller than a change
/// record plus this allowance, so the allowance is always affordable.
pub(crate) const PAGE_OVERHEAD: usize = 512;

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct StateLimits {
    pub request_bytes: usize,
    pub state_bytes: usize,
    pub history_bytes: usize,
    pub history_records: usize,
    pub change_bytes: usize,
    pub snapshot_bytes: usize,
    pub total_snapshot_bytes: usize,
    /// Live handles one principal (view, or owner-bound client) may hold.
    pub handles: usize,
    /// Live handles across every principal together; the global ceiling
    /// above the per-principal `handles` budget.
    pub max_handles: usize,
    /// Rejected-command faults retained across every principal together.
    /// Each principal is separately held to `handles` faults.
    pub max_faults: usize,
    pub mutations: usize,
    pub depth: usize,
    pub nodes: usize,
    pub page_bytes: usize,
    pub page_items: usize,
    pub handle_age: Duration,
}
impl Default for StateLimits {
    fn default() -> Self {
        Self {
            request_bytes: 2 << 20,
            state_bytes: 1 << 20,
            history_bytes: 1 << 20,
            history_records: 256,
            change_bytes: 32768,
            snapshot_bytes: 2 << 20,
            total_snapshot_bytes: 8 << 20,
            handles: 64,
            max_handles: 256,
            max_faults: 256,
            mutations: 128,
            depth: 32,
            nodes: 32768,
            page_bytes: 65536,
            page_items: 256,
            handle_age: Duration::from_secs(60),
        }
    }
}

/// `StateLimits` is `#[non_exhaustive]`: start from [`StateLimits::default`]
/// and narrow it with these setters rather than struct-update syntax.
impl StateLimits {
    /// Largest command envelope accepted at `operations`.
    pub fn with_request_bytes(mut self, bytes: usize) -> Self {
        self.request_bytes = bytes;
        self
    }
    /// Largest canonical encoding of the whole committed tree.
    pub fn with_state_bytes(mut self, bytes: usize) -> Self {
        self.state_bytes = bytes;
        self
    }
    /// Total retained change-history bytes.
    pub fn with_history_bytes(mut self, bytes: usize) -> Self {
        self.history_bytes = bytes;
        self
    }
    /// Number of retained change records before the oldest is dropped.
    pub fn with_history_records(mut self, records: usize) -> Self {
        self.history_records = records;
        self
    }
    /// Largest single change record; also the floor for a page budget.
    pub fn with_change_bytes(mut self, bytes: usize) -> Self {
        self.change_bytes = bytes;
        self
    }
    /// Largest single pinned snapshot.
    pub fn with_snapshot_bytes(mut self, bytes: usize) -> Self {
        self.snapshot_bytes = bytes;
        self
    }
    /// Total bytes pinned across every live handle.
    pub fn with_total_snapshot_bytes(mut self, bytes: usize) -> Self {
        self.total_snapshot_bytes = bytes;
        self
    }
    /// Live handles admitted at once for one principal, so a single view or
    /// owner cannot exhaust the handles every other view needs.
    pub fn with_handles(mut self, handles: usize) -> Self {
        self.handles = handles;
        self
    }
    /// Live handles admitted at once across all principals.
    pub fn with_max_handles(mut self, handles: usize) -> Self {
        self.max_handles = handles;
        self
    }
    /// Rejected-command faults retained across all principals; past this the
    /// globally oldest fault is dropped (and then reads as `Fault::Closed`).
    pub fn with_max_faults(mut self, faults: usize) -> Self {
        self.max_faults = faults;
        self
    }
    /// Mutations admitted in one atomic batch.
    pub fn with_mutations(mut self, mutations: usize) -> Self {
        self.mutations = mutations;
        self
    }
    /// Deepest path admitted in the committed tree.
    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }
    /// Most `Value` nodes admitted in the committed tree.
    pub fn with_nodes(mut self, nodes: usize) -> Self {
        self.nodes = nodes;
        self
    }
    /// Largest page a reader may request.
    pub fn with_page_bytes(mut self, bytes: usize) -> Self {
        self.page_bytes = bytes;
        self
    }
    /// Most entries a reader may request in one page.
    pub fn with_page_items(mut self, items: usize) -> Self {
        self.page_items = items;
        self
    }
    /// How long an unused handle survives before it is reaped.
    pub fn with_handle_age(mut self, age: Duration) -> Self {
        self.handle_age = age;
        self
    }

    /// The envelope allowance for a command written to `operations`.
    ///
    /// Bounding the request happens before recursive Serde conversion, and
    /// envelope nesting is separate from the state tree's own depth — a valid
    /// tree sitting exactly at the depth limit must still be settable.
    pub(crate) fn envelope(&self) -> Self {
        let mut limits = self.clone();
        limits.state_bytes = limits.request_bytes;
        limits.depth = limits.depth.saturating_add(8);
        limits.nodes = limits.nodes.saturating_mul(2).saturating_add(1024);
        limits
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> Fault {
    Fault::Invalid {
        message: message.into(),
    }
}
pub(crate) fn limit(message: impl Into<String>) -> Fault {
    Fault::ResourceLimit {
        message: message.into(),
    }
}

/// Canonical encoded size of a value.
///
/// This is the provider's most expensive primitive. Under test it also tallies
/// how many bytes have been encoded in total, so a test can assert that adding
/// mutations to a batch does not multiply the work done on the whole tree.
pub(crate) fn size<T: Serialize>(value: &T) -> Result<usize, Fault> {
    let value = to_value(value).map_err(|_| limit("value conversion bounds"))?;
    let codec = ValueCodec::new(CodecProfile::ValueJson)
        .canonical()
        .map_err(|_| limit("canonical codec unavailable"))?;
    let bytes = codec
        .encode(&value, &codec.profile.format())
        .map(|b| b.len())
        .map_err(|_| limit("encoded value bounds"))?;
    #[cfg(test)]
    counter::record(bytes);
    Ok(bytes)
}

/// Structural bounds only: depth and node count. Cheap, and safe to run on a
/// caller-supplied value before the state lock is taken.
pub(crate) fn validate_shape(value: &Value, limits: &StateLimits) -> Result<(), Fault> {
    fn visit(
        value: &Value,
        depth: usize,
        count: &mut usize,
        limits: &StateLimits,
    ) -> Result<(), Fault> {
        *count += 1;
        if depth > limits.depth || *count > limits.nodes {
            return Err(limit("state depth or node count"));
        }
        match value {
            Value::Map(m) => {
                for v in m.values() {
                    visit(v, depth + 1, count, limits)?;
                }
            }
            Value::Array(a) => {
                for v in a {
                    visit(v, depth + 1, count, limits)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    visit(value, 0, &mut 0, limits)
}

/// Structural bounds plus the canonical byte budget. Run this once per commit
/// on the proposed tree, never once per mutation.
pub(crate) fn validate(value: &Value, limits: &StateLimits) -> Result<(), Fault> {
    validate_shape(value, limits)?;
    if size(value)? > limits.state_bytes {
        return Err(limit("state bytes"));
    }
    Ok(())
}

/// Per-thread tally, so concurrently running tests cannot inflate each
/// other's counts (`#[tokio::test]` runs each test on its own thread).
#[cfg(test)]
pub(crate) mod counter {
    use std::cell::Cell;

    thread_local! {
        static ENCODED: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn record(bytes: usize) {
        ENCODED.with(|c| c.set(c.get() + bytes));
    }
    pub(crate) fn reset() {
        ENCODED.with(|c| c.set(0));
    }
    pub(crate) fn encoded_bytes() -> usize {
        ENCODED.with(Cell::get)
    }
}
