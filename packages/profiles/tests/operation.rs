#![cfg(feature = "host")]
use std::time::Duration;
use structfs_profiles::{OperationHandle, Phase};
use structfs_service::{CleanupSupervisor, OwnerLimits};

#[tokio::test]
async fn cancel_is_not_completion_close_retains_noncooperative_charge() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor
        .owner(
            OwnerLimits::default()
                .with_resources(8)
                .with_retained_bytes(8),
        )
        .unwrap();
    let (send, wait) = tokio::sync::oneshot::channel();
    let operation = OperationHandle::start(&owner.handle(), "one".into(), 8, move |_| async move {
        wait.await.unwrap();
        Ok(vec![1; 8])
    })
    .unwrap();
    assert!(
        OperationHandle::start(&owner.handle(), "overflow".into(), 1, |_| async {
            Ok(vec![])
        })
        .is_err()
    );
    operation.cancel();
    assert!(operation.status().cancel_requested);
    assert!(!operation.status().joined);
    operation.close();
    assert!(!owner.join(Duration::from_millis(1)).await.is_quiescent());
    assert!(owner
        .handle()
        .report()
        .remaining
        .iter()
        .any(|r| r.bytes == 8));
    send.send(()).unwrap();
    assert!(owner.join(Duration::from_secs(1)).await.is_quiescent());
    assert!(operation.status().joined);
    assert!(matches!(operation.status().phase, Phase::Completed));
    assert!(operation.result().is_err());
}
#[tokio::test]
async fn bounded_result_and_terminal_failure_are_repeatable() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(Default::default()).unwrap();
    for size in [4, 5] {
        let operation =
            OperationHandle::start(&owner.handle(), "one".into(), 4, move |_| async move {
                Ok(vec![7; size])
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !operation.status().joined {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if size == 4 {
            assert_eq!(operation.result().unwrap(), Some(vec![7; 4]));
            assert_eq!(operation.result().unwrap(), Some(vec![7; 4]));
        } else {
            assert!(matches!(operation.status().phase, Phase::Failed));
            assert!(operation.result().is_err());
        }
        operation.close();
    }
    assert!(owner.join(Duration::from_secs(1)).await.is_quiescent());
}
