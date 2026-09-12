//! Shared routed-call admission and runtime-specific session admission.
use crate::block::BlockId;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use structfs_core_store::Error;
pub use structfs_service::{CallBudgetSnapshot, CallLimits, CallMetrics, CallUsage};
pub type CallBudget = structfs_service::CallBudget<BlockId>;
pub(crate) type CallCharge = structfs_service::Lease;
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
