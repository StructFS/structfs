//! Nonblocking admission for routed calls. Charges last until response or
//! abandonment, including calls already dequeued by the guest.
use crate::block::BlockId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use structfs_core_store::{Error, Path, Value};

/// Limits on outstanding calls, shared across runtimes when desired.
#[derive(Clone, Debug)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallUsage {
    pub calls: usize,
    pub bytes: usize,
}
#[derive(Default)]
struct Usage {
    total: CallUsage,
    blocks: HashMap<BlockId, CallUsage>,
}
/// A shared, fail-fast budget. Each block has its own ceiling in addition
/// to the global limit; this bounds monopolization, not scheduling latency.
pub struct CallBudget {
    limits: CallLimits,
    usage: Mutex<Usage>,
}
impl CallBudget {
    pub fn new(limits: CallLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            usage: Mutex::new(Usage::default()),
        })
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
        let limit = self.limits.bytes.min(self.limits.bytes_per_block);
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
                return Err(Error::overloaded("request exceeds call byte budget"));
            }
        }
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let local = usage.blocks.get(block).copied().unwrap_or_default();
        if usage.total.calls >= self.limits.calls
            || local.calls >= self.limits.calls_per_block
            || bytes > self.limits.bytes.saturating_sub(usage.total.bytes)
            || bytes > self.limits.bytes_per_block.saturating_sub(local.bytes)
        {
            return Err(Error::overloaded("outstanding call budget exhausted"));
        }
        usage.total.calls += 1;
        usage.total.bytes += bytes;
        let local = usage.blocks.entry(block.clone()).or_default();
        local.calls += 1;
        local.bytes += bytes;
        Ok(CallCharge {
            budget: self.clone(),
            block: block.clone(),
            bytes,
        })
    }
}
pub(crate) struct CallCharge {
    budget: Arc<CallBudget>,
    block: BlockId,
    bytes: usize,
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
#[derive(Clone, Debug)]
pub struct SessionLimits {
    pub sessions: usize,
    pub sessions_per_tenant: usize,
    pub guest_slots: usize,
    pub memory_bytes: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionUsage {
    pub sessions: usize,
    pub guest_slots: usize,
    pub memory_bytes: usize,
}
#[derive(Default)]
struct Sessions {
    total: SessionUsage,
    tenants: HashMap<String, usize>,
}
pub struct SessionBudget {
    limits: SessionLimits,
    state: Mutex<Sessions>,
}
impl SessionBudget {
    pub fn new(limits: SessionLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            state: Mutex::new(Sessions::default()),
        })
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
        let tenant_count = state.tenants.get(tenant).copied().unwrap_or(0);
        if guest_slots == 0
            || memory_bytes == 0
            || state.total.sessions >= self.limits.sessions
            || tenant_count >= self.limits.sessions_per_tenant
            || guest_slots
                > self
                    .limits
                    .guest_slots
                    .saturating_sub(state.total.guest_slots)
            || memory_bytes
                > self
                    .limits
                    .memory_bytes
                    .saturating_sub(state.total.memory_bytes)
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
