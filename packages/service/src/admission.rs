//! Nonblocking admission for routed calls. Charges last until response or
//! abandonment, including calls already dequeued by the guest.
use crate::Lease;
use serde::{Deserialize, Serialize};
use std::hash::Hash;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use structfs_core_store::{Error, Path, Record, Value};

/// Limits on outstanding calls, shared across runtimes when desired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallLimits {
    pub calls: usize,
    pub bytes: usize,
    pub calls_per_block: usize,
    pub bytes_per_block: usize,
}
impl Default for CallLimits {
    fn default() -> Self {
        Self {
            calls: 16_384,
            bytes: 64 * 1024 * 1024,
            calls_per_block: 256,
            bytes_per_block: 8 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallUsage {
    pub calls: usize,
    pub bytes: usize,
}
/// Cumulative admission measurements. Bytes are logical payload weight, not RSS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallMetrics {
    pub admitted: u64,
    pub rejected: u64,
    pub admitted_bytes: u64,
    pub peak_calls: usize,
    pub peak_bytes: usize,
}
/// An atomic view of this budget, not an aggregate snapshot of its ancestors.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallBudgetSnapshot {
    pub revision: u64,
    pub limits: CallLimits,
    pub usage: CallUsage,
    pub metrics: CallMetrics,
}
struct Usage<I> {
    revision: u64,
    limits: CallLimits,
    metrics: CallMetrics,
    total: CallUsage,
    blocks: HashMap<I, CallUsage>,
}
/// A shared, fail-fast budget. Each block has its own ceiling in addition
/// to the global limit; this bounds monopolization, not scheduling latency.
pub struct CallBudget<I: Clone + Eq + Hash + Send + Sync + 'static = String> {
    parent: Option<Arc<CallBudget<I>>>,
    usage: Mutex<Usage<I>>,
}
impl<I: Clone + Eq + Hash + Send + Sync + 'static> CallBudget<I> {
    pub fn new(limits: CallLimits) -> Arc<Self> {
        Self::build(limits, None)
    }
    fn build(limits: CallLimits, parent: Option<Arc<Self>>) -> Arc<Self> {
        Arc::new(Self {
            parent,
            usage: Mutex::new(Usage {
                limits,
                revision: 0,
                metrics: CallMetrics::default(),
                total: CallUsage::default(),
                blocks: HashMap::new(),
            }),
        })
    }
    /// Create a request or tenant budget that also charges this budget and
    /// every ancestor. Retain the returned handle to update or inspect it.
    pub fn child(self: &Arc<Self>, limits: CallLimits) -> Arc<Self> {
        Self::build(limits, Some(self.clone()))
    }
    /// Atomically replace admission policy without forgetting live charges.
    /// Existing calls are grandfathered; new calls must fit the new limits.
    pub fn set_limits(&self, limits: CallLimits) {
        let mut state = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        state.limits = limits;
        state.revision = state.revision.saturating_add(1);
    }
    pub fn limits(&self) -> CallLimits {
        self.usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .limits
            .clone()
    }
    /// Child-to-root observations. Each budget snapshot is atomic; the whole
    /// hierarchy is not a transaction. Every listed ceiling is enforced.
    pub fn hierarchy(&self) -> Vec<CallBudgetSnapshot> {
        let mut result = vec![self.snapshot()];
        let mut parent = self.parent.as_deref();
        while let Some(budget) = parent {
            result.push(budget.snapshot());
            parent = budget.parent.as_deref();
        }
        result
    }
    pub fn snapshot(&self) -> CallBudgetSnapshot {
        let state = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        CallBudgetSnapshot {
            revision: state.revision,
            limits: state.limits.clone(),
            usage: state.total,
            metrics: state.metrics,
        }
    }
    pub fn metrics(&self) -> CallMetrics {
        self.usage.lock().unwrap_or_else(|e| e.into_inner()).metrics
    }
    pub fn usage(&self) -> CallUsage {
        self.usage.lock().unwrap_or_else(|e| e.into_inner()).total
    }
    pub fn acquire(self: &Arc<Self>, block: &I, path: &Path, data: &Value) -> Result<Lease, Error> {
        let limits = self.limits();
        let limit = limits.bytes.min(limits.bytes_per_block);
        // Logical retained payload weight: Value nodes, keys, string/byte
        // contents, and path text. This is not an allocator/RSS measurement.
        let mut bytes = path.to_string().len().saturating_add(128);
        let mut pending = vec![data];
        while let Some(value) = pending.pop() {
            bytes = bytes.saturating_add(std::mem::size_of::<Value>());
            match value {
                Value::String(s) => bytes = bytes.saturating_add(s.len()),
                Value::Bytes(b) => bytes = bytes.saturating_add(b.len()),
                Value::Array(a) => pending.extend(a),
                Value::Map(m) => {
                    for (k, v) in m {
                        bytes = bytes.saturating_add(k.len());
                        pending.push(v);
                    }
                }
                _ => {}
            }
            if bytes > limit {
                let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
                usage.metrics.rejected = usage.metrics.rejected.saturating_add(1);
                return Err(Error::overloaded("request exceeds call byte budget"));
            }
        }
        self.acquire_bytes(block, bytes)
    }
    /// Charge a parsed or raw request without decoding opaque bytes.
    pub fn acquire_record(
        self: &Arc<Self>,
        key: &I,
        path: &Path,
        data: Option<&Record>,
    ) -> Result<Lease, Error> {
        match data {
            Some(Record::Parsed(value)) => self.acquire(key, path, value),
            Some(Record::Raw { bytes, .. }) => self.acquire_bytes(
                key,
                path.to_string()
                    .len()
                    .saturating_add(128)
                    .saturating_add(bytes.len()),
            ),
            None => self.acquire(key, path, &Value::Null),
            _ => Err(Error::store("admission", "acquire", "unsupported record")),
        }
    }
    pub fn acquire_bytes(self: &Arc<Self>, block: &I, bytes: usize) -> Result<Lease, Error> {
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let limits = &usage.limits;
        let local = usage.blocks.get(block).copied().unwrap_or_default();
        if usage.total.calls >= limits.calls
            || local.calls >= limits.calls_per_block
            || bytes > limits.bytes.saturating_sub(usage.total.bytes)
            || bytes > limits.bytes_per_block.saturating_sub(local.bytes)
        {
            usage.metrics.rejected = usage.metrics.rejected.saturating_add(1);
            return Err(Error::overloaded("outstanding call budget exhausted"));
        }
        // All acquisition locks run from child to ancestor; the hierarchy is
        // immutable and acyclic. A parent rejection leaves this child uncharged.
        let parent = match &self.parent {
            Some(parent) => match parent.acquire_bytes(block, bytes) {
                Ok(charge) => Some(Box::new(charge)),
                Err(error) => {
                    usage.metrics.rejected = usage.metrics.rejected.saturating_add(1);
                    return Err(error);
                }
            },
            None => None,
        };
        usage.total.calls += 1;
        usage.total.bytes += bytes;
        usage.metrics.admitted = usage.metrics.admitted.saturating_add(1);
        usage.metrics.admitted_bytes = usage.metrics.admitted_bytes.saturating_add(bytes as u64);
        usage.metrics.peak_calls = usage.metrics.peak_calls.max(usage.total.calls);
        usage.metrics.peak_bytes = usage.metrics.peak_bytes.max(usage.total.bytes);
        let local = usage.blocks.entry(block.clone()).or_default();
        local.calls += 1;
        local.bytes += bytes;
        Ok(Lease::new(CallCharge {
            budget: self.clone(),
            block: block.clone(),
            bytes,
            _parent: parent,
        }))
    }
}
struct CallCharge<I: Clone + Eq + Hash + Send + Sync + 'static> {
    budget: Arc<CallBudget<I>>,
    block: I,
    bytes: usize,
    _parent: Option<Box<Lease>>,
}
impl<I: Clone + Eq + Hash + Send + Sync + 'static> Drop for CallCharge<I> {
    fn drop(&mut self) {
        let mut usage = self.budget.usage.lock().unwrap_or_else(|e| e.into_inner());
        usage.total.calls -= 1;
        usage.total.bytes -= self.bytes;
        if let Some(local) = usage.blocks.get_mut(&self.block) {
            local.calls -= 1;
            local.bytes -= self.bytes;
            if local.calls == 0 {
                usage.blocks.remove(&self.block);
            }
        }
    }
}
