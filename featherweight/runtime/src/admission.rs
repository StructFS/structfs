//! Shared routed-call admission.
use crate::block::BlockId;
pub use structfs_service::{CallBudgetSnapshot, CallLimits, CallMetrics, CallUsage};
pub type CallBudget = structfs_service::CallBudget<BlockId>;
pub(crate) type CallCharge = structfs_service::Lease;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn call_limits(calls: usize) -> CallLimits {
        CallLimits::default()
            .with_calls(calls)
            .with_calls_per_block(calls)
    }

    #[test]
    fn request_hierarchy_retains_charges_across_policy_changes() {
        let global = CallBudget::shared(call_limits(2));
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
        assert_eq!(global.usage(), CallUsage::new(2, 200));
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
        let global = CallBudget::shared(call_limits(4));
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
}
