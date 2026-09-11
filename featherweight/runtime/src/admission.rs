//! Nonblocking admission for routed calls. Charges last until response or
//! abandonment, including calls already dequeued by the guest.
use crate::block::BlockId;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use structfs_core_store::{Error, Path, Value};

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
struct Usage {
    revision: u64,
    limits: CallLimits,
    metrics: CallMetrics,
    total: CallUsage,
    blocks: HashMap<BlockId, CallUsage>,
}
/// A shared, fail-fast budget. Each block has its own ceiling in addition
/// to the global limit; this bounds monopolization, not scheduling latency.
pub struct CallBudget {
    parent: Option<Arc<CallBudget>>,
    usage: Mutex<Usage>,
}
impl CallBudget {
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
    pub(crate) fn acquire(
        self: &Arc<Self>,
        block: &BlockId,
        path: &Path,
        data: &Value,
    ) -> Result<CallCharge, Error> {
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
    fn acquire_bytes(self: &Arc<Self>, block: &BlockId, bytes: usize) -> Result<CallCharge, Error> {
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
        Ok(CallCharge {
            budget: self.clone(),
            block: block.clone(),
            bytes,
            _parent: parent,
        })
    }
}
pub(crate) struct CallCharge {
    budget: Arc<CallBudget>,
    block: BlockId,
    bytes: usize,
    _parent: Option<Box<CallCharge>>,
}
impl Drop for CallCharge {
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

/// Admission for fresh HTTP/execution sessions. Reserve worst-case guest
/// linear memory for the whole assembly, including lazy blocks, before start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLimits {
    pub sessions: usize,
    pub sessions_per_tenant: usize,
    pub guest_slots: usize,
    pub memory_bytes: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUsage {
    pub sessions: usize,
    pub guest_slots: usize,
    pub memory_bytes: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionBudgetSnapshot {
    pub revision: u64,
    pub limits: SessionLimits,
    /// Reserved capacity, not measured guest-memory usage.
    pub usage: SessionUsage,
}
struct Sessions {
    revision: u64,
    limits: SessionLimits,
    total: SessionUsage,
    tenants: HashMap<String, usize>,
}
pub struct SessionBudget {
    state: Mutex<Sessions>,
}
impl SessionBudget {
    pub fn new(limits: SessionLimits) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(Sessions {
                limits,
                revision: 0,
                total: SessionUsage::default(),
                tenants: HashMap::new(),
            }),
        })
    }
    /// Update policy atomically. Existing reservations remain charged and
    /// valid; admissions resume once usage fits the updated ceilings.
    pub fn set_limits(&self, limits: SessionLimits) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.limits = limits;
        state.revision = state.revision.saturating_add(1);
    }
    pub fn limits(&self) -> SessionLimits {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .limits
            .clone()
    }
    pub fn snapshot(&self) -> SessionBudgetSnapshot {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        SessionBudgetSnapshot {
            revision: state.revision,
            limits: state.limits.clone(),
            usage: state.total,
        }
    }
    pub fn usage(&self) -> SessionUsage {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).total
    }
    pub fn admit(
        self: &Arc<Self>,
        tenant: &str,
        guest_slots: usize,
        memory_bytes: usize,
    ) -> Result<SessionPermit, Error> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let limits = &state.limits;
        let tenant_count = state.tenants.get(tenant).copied().unwrap_or(0);
        if guest_slots == 0
            || memory_bytes == 0
            || state.total.sessions >= limits.sessions
            || tenant_count >= limits.sessions_per_tenant
            || guest_slots > limits.guest_slots.saturating_sub(state.total.guest_slots)
            || memory_bytes > limits.memory_bytes.saturating_sub(state.total.memory_bytes)
        {
            return Err(Error::overloaded("execution session budget exhausted"));
        }
        state.total.sessions += 1;
        state.total.guest_slots += guest_slots;
        state.total.memory_bytes += memory_bytes;
        *state.tenants.entry(tenant.into()).or_default() += 1;
        Ok(SessionPermit {
            budget: self.clone(),
            tenant: tenant.into(),
            guest_slots,
            memory_bytes,
        })
    }
}
/// Keep this permit until teardown has joined every guest, including when
/// the caller disconnects. Dropping it releases the reserved capacity.
pub struct SessionPermit {
    budget: Arc<SessionBudget>,
    tenant: String,
    guest_slots: usize,
    memory_bytes: usize,
}
impl Drop for SessionPermit {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().unwrap_or_else(|e| e.into_inner());
        state.total.sessions -= 1;
        state.total.guest_slots -= self.guest_slots;
        state.total.memory_bytes -= self.memory_bytes;
        let count = state.tenants.get_mut(&self.tenant).unwrap();
        *count -= 1;
        if *count == 0 {
            state.tenants.remove(&self.tenant);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call_limits(calls: usize) -> CallLimits {
        CallLimits {
            calls,
            calls_per_block: calls,
            ..CallLimits::default()
        }
    }

    #[test]
    fn request_hierarchy_retains_charges_across_policy_changes() {
        let global = CallBudget::new(call_limits(2));
        let tenant = global.child(call_limits(2));
        let request = tenant.child(call_limits(1));
        let peer = global.child(call_limits(2));
        let block = BlockId::new();
        let charge = request.acquire_bytes(&block, 100).unwrap();
        assert!(request.acquire_bytes(&block, 100).is_err());
        assert_eq!(global.usage().calls, 1);
        request.set_limits(call_limits(2));
        let second = request.acquire_bytes(&block, 100).unwrap();
        assert!(peer.acquire_bytes(&BlockId::new(), 100).is_err());
        assert_eq!(peer.usage(), CallUsage::default());
        global.set_limits(call_limits(0));
        assert_eq!(
            global.usage(),
            CallUsage {
                calls: 2,
                bytes: 200
            }
        );
        drop(charge);
        assert!(peer.acquire_bytes(&block, 100).is_err());
        drop(second);
        for budget in [&global, &tenant, &request] {
            assert_eq!(budget.usage(), CallUsage::default());
            assert_eq!(budget.metrics().admitted, 2);
            assert_eq!(budget.metrics().admitted_bytes, 200);
            assert_eq!(budget.metrics().peak_calls, 2);
        }
        global.set_limits(call_limits(1));
        assert!(peer.acquire_bytes(&block, 100).is_ok());
        assert_eq!(global.usage(), CallUsage::default());
    }

    #[test]
    fn concurrent_requests_cannot_exceed_shared_capacity() {
        let global = CallBudget::new(call_limits(4));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let request = global.child(call_limits(4));
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let charge = request.acquire_bytes(&BlockId::new(), 100);
                    barrier.wait();
                    drop(charge);
                });
            }
        });
        assert_eq!(global.metrics().admitted, 4);
        assert_eq!(global.metrics().rejected, 12);
        assert_eq!(global.usage(), CallUsage::default());
    }

    #[test]
    fn session_reload_preserves_live_reservations() {
        let limits = SessionLimits {
            sessions: 2,
            sessions_per_tenant: 2,
            guest_slots: 4,
            memory_bytes: 4096,
        };
        let budget = SessionBudget::new(limits.clone());
        let permit = budget.admit("tenant", 2, 2048).unwrap();
        budget.set_limits(SessionLimits {
            memory_bytes: 1,
            ..limits.clone()
        });
        assert_eq!(budget.usage().memory_bytes, 2048);
        assert!(budget.admit("peer", 1, 1).is_err());
        drop(permit);
        assert_eq!(budget.usage(), SessionUsage::default());
        budget.set_limits(limits.clone());
        assert_eq!(budget.limits(), limits);
        assert!(budget.admit("peer", 4, 4096).is_ok());
    }

    #[test]
    fn session_limits_preserve_peer_capacity_and_release_all_charges() {
        let budget = SessionBudget::new(SessionLimits {
            sessions: 3,
            sessions_per_tenant: 1,
            guest_slots: 4,
            memory_bytes: 4096,
        });
        let a = budget.admit("a", 2, 2048).unwrap();
        assert!(budget.admit("a", 1, 1).is_err());
        assert!(budget.admit("b", 1, 2049).is_err());
        let b = budget.admit("b", 2, 2048).unwrap();
        assert!(budget.admit("c", 1, 1).is_err());
        drop(a);
        drop(b);
        assert_eq!(budget.usage(), SessionUsage::default());
        assert!(budget.admit("c", 4, 4096).is_ok());
        assert_eq!(budget.usage(), SessionUsage::default());
    }
}
