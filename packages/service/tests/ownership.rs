use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use structfs_core_store::{path, DetachedFuture, Error, MemoryStore, Record};
use structfs_service::*;
const WAIT: Duration = Duration::from_secs(2);
fn mount(service: Arc<dyn Service>, budget: Arc<CallBudget>) -> Mount {
    Mount::new(
        path!("svc"),
        path!(""),
        service,
        Arc::new(BudgetAdmission {
            budget,
            key: "svc".into(),
        }),
    )
}
#[tokio::test]
async fn owner_drop_closes_admission_and_supervisor_joins_cleanup_once() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let handle = owner.handle();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let (send, recv) = tokio::sync::oneshot::channel();
    let registration = handle
        .register(ResourceKind::Registration, 4, move || async move {
            c.fetch_add(1, Ordering::SeqCst);
            recv.await.unwrap();
            Ok(())
        })
        .unwrap();
    drop(owner);
    assert!(handle.track(ResourceKind::Task, 0).is_err());
    assert!(supervisor.owner(OwnerLimits::default()).is_err());
    registration.release();
    let report = supervisor.close(Duration::ZERO).await;
    assert!(!report[0].is_quiescent());
    assert_eq!(report[0].remaining[0].bytes, 4);
    send.send(()).unwrap();
    let report = supervisor.close(WAIT).await;
    assert!(report[0].is_quiescent());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(supervisor.owner(OwnerLimits::default()).is_err());
}
#[tokio::test]
async fn bounded_resources_and_children_do_not_cancel_unrelated_owners() {
    let supervisor = CleanupSupervisor::new(4).unwrap();
    let parent = supervisor
        .owner(OwnerLimits {
            resources: 1,
            retained_bytes: 4,
        })
        .unwrap();
    let child = parent
        .handle()
        .child(&supervisor, OwnerLimits::default())
        .unwrap();
    let independent = supervisor.owner(OwnerLimits::default()).unwrap();
    let task = child
        .handle()
        .spawn(|cancel| async move {
            cancel.cancelled().await;
            Ok(())
        })
        .unwrap();
    assert!(parent.handle().track(ResourceKind::Provider, 0).is_err());
    parent.cancel();
    assert!(child.handle().track(ResourceKind::Provider, 0).is_err());
    assert!(parent.close(WAIT).await.is_quiescent());
    assert!(child
        .handle()
        .report()
        .remaining
        .iter()
        .all(|r| r.id != task));
    assert!(independent.handle().ensure_open().is_ok());
    let retained = independent
        .handle()
        .track(ResourceKind::Retained, 4)
        .unwrap();
    assert!(!independent.close(Duration::ZERO).await.is_quiescent());
    drop(retained);
    assert!(independent.close(WAIT).await.is_quiescent());
}
#[tokio::test]
async fn task_panics_and_failed_cleanup_are_reported_until_reconciled() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let handle = owner.handle();
    let task = handle
        .spawn(|_| async {
            panic!("provider panic");
            #[allow(unreachable_code)]
            Ok(())
        })
        .unwrap();
    let cleanup = handle
        .register(ResourceKind::Registration, 7, || async {
            Err(Error::store("test", "release", "unknown outcome"))
        })
        .unwrap();
    owner.cancel();
    tokio::time::timeout(WAIT, async {
        while handle.report().failures != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let report = owner.close(Duration::ZERO).await;
    assert!(!report.is_quiescent());
    assert_eq!(report.remaining.len(), 2);
    assert!(report.remaining.iter().all(|r| r.failed));
    handle.acknowledge_failure(task).unwrap();
    handle.acknowledge_failure(cleanup.id()).unwrap();
    assert!(owner.close(WAIT).await.is_quiescent());
    assert!(handle.acknowledge_failure(task).is_err());
}
#[tokio::test]
async fn revocation_applies_to_cloned_clients_and_replacement_mounts() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let router = Router::new(vec![]).unwrap();
    let budget = CallBudget::new(CallLimits::default());
    let provider = Arc::new(ImmediateStore::new(MemoryStore::new()));
    let registration = router
        .register(&owner.handle(), mount(provider.clone(), budget.clone()))
        .unwrap();
    let client = router
        .client()
        .scoped(&path!("svc"), Permissions::READ_WRITE);
    client
        .write(&path!("x"), Record::parsed(1i64.into()))
        .await
        .unwrap();
    let clone = client.clone();
    registration.release();
    assert!(clone.read(&path!("x")).await.is_err());
    registration.close(WAIT).await;
    let replacement = router
        .register(&owner.handle(), mount(provider, budget.clone()))
        .unwrap();
    assert_ne!(registration.id(), replacement.id());
    drop(registration);
    assert!(clone.read(&path!("x")).await.unwrap().is_some());
    assert!(owner.close(WAIT).await.is_quiescent());
    assert!(clone.read(&path!("x")).await.is_err());
    assert_eq!(budget.usage().calls, 0);
}
struct Noncooperative {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
impl Service for Noncooperative {
    fn call(&self, context: CallContext, _: Operation) -> DetachedFuture<Response> {
        let release = self.release.clone();
        let started = self.started.clone();
        let owner = context.owner().unwrap().clone();
        owner
            .spawn(move |_| async move {
                let _context = context;
                started.notify_one();
                release.notified().await;
                Ok(())
            })
            .unwrap();
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn abandoned_provider_work_stays_owned_and_charged_after_caller_returns() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let budget = CallBudget::new(CallLimits::default());
    let client = Router::new(vec![mount(
        owner.handle().service(Arc::new(Noncooperative {
            started: started.clone(),
            release: release.clone(),
        })),
        budget.clone(),
    )])
    .unwrap()
    .client();
    let call = tokio::spawn(async move { client.read(&path!("svc/x")).await });
    started.notified().await;
    call.abort();
    let _ = call.await;
    let report = owner.close(Duration::ZERO).await;
    assert_eq!(report.remaining.len(), 2); // provider reservation and supervised task
    assert_eq!(budget.usage().calls, 1);
    release.notify_one();
    assert!(owner.close(WAIT).await.is_quiescent());
    assert_eq!(budget.usage().calls, 0);
}
#[tokio::test]
async fn retained_results_are_bounded_and_abandoned_delivery_releases_storage() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor
        .owner(OwnerLimits {
            resources: 2,
            retained_bytes: 4,
        })
        .unwrap();
    assert!(RetainedBytes::new(&owner.handle(), vec![0; 5]).is_err());
    let result = RetainedBytes::new(&owner.handle(), vec![1, 2, 3, 4]).unwrap();
    assert_eq!(result.snapshot().unwrap(), vec![1, 2, 3, 4]);
    assert!(RetainedBytes::new(&owner.handle(), vec![1]).is_err());
    let (send, recv) = tokio::sync::oneshot::channel();
    drop(recv);
    drop(send.send(result));
    assert!(owner.close(WAIT).await.is_quiescent());
}
#[tokio::test]
async fn tail_backpressure_cursor_errors_terminal_pages_and_release() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let tail = Arc::new(OwnedTail::new(&owner.handle(), 2, 4).unwrap());
    let cancel = CancelToken::new();
    assert_eq!(tail.push(vec![1, 2]).unwrap(), 0);
    assert_eq!(tail.push(vec![3, 4]).unwrap(), 1);
    assert!(matches!(tail.push(vec![5]), Err(Error::Overloaded { .. })));
    let first = tail.read(0, 1, &cancel).await.unwrap();
    assert_eq!(first.next, 1);
    assert!(!first.done);
    tail.acknowledge(1).unwrap();
    assert!(tail.read(0, 1, &cancel).await.is_err());
    assert!(tail.read(3, 1, &cancel).await.is_err());
    assert_eq!(tail.push(vec![5]).unwrap(), 2);
    tail.finish();
    assert!(!tail.read(1, 1, &cancel).await.unwrap().done);
    assert!(tail.read(2, 1, &cancel).await.unwrap().done);
    assert!(tail.push(vec![]).is_err());
    assert!(owner.close(WAIT).await.is_quiescent());
    assert!(tail.read(2, 1, &cancel).await.is_err());
}
#[tokio::test]
async fn owner_close_wakes_parked_tail_reader_and_revokes_delivered_result() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let result = RetainedBytes::new(&owner.handle(), vec![9]).unwrap();
    let tail = Arc::new(OwnedTail::new(&owner.handle(), 1, 4).unwrap());
    let read = tokio::spawn(async move { tail.read(0, 1, &CancelToken::new()).await });
    assert!(owner.close(WAIT).await.is_quiescent());
    assert!(read.await.unwrap().is_err());
    assert!(result.snapshot().is_err());
}

#[tokio::test]
async fn request_client_ownership_is_monotonic_and_does_not_revoke_shared_service() {
    let supervisor = CleanupSupervisor::new(3).unwrap();
    let service = supervisor.owner(OwnerLimits::default()).unwrap();
    let request = supervisor.owner(OwnerLimits::default()).unwrap();
    let unrelated = supervisor.owner(OwnerLimits::default()).unwrap();
    let router = Router::new(vec![]).unwrap();
    let budget = CallBudget::new(CallLimits::default());
    let _registration = router
        .register(
            &service.handle(),
            mount(
                Arc::new(ImmediateStore::new(MemoryStore::new())),
                budget.clone(),
            ),
        )
        .unwrap();
    let caller = router.client().owned_by(&request.handle());
    request.cancel();
    let attempted_escape = caller
        .owned_by(&unrelated.handle())
        .scoped(&path!("svc"), Permissions::READ_WRITE)
        .with_context(CallContext::default());
    assert!(attempted_escape
        .read(&path!("x"))
        .await
        .unwrap_err()
        .is_cancelled());
    assert_eq!(budget.metrics().admitted, 0);
    assert!(router
        .client()
        .owned_by(&unrelated.handle())
        .read(&path!("svc/x"))
        .await
        .is_ok());
    assert!(service.handle().ensure_open().is_ok());
    assert!(supervisor
        .close(WAIT)
        .await
        .iter()
        .all(CloseReport::is_quiescent));
}

#[tokio::test]
async fn abandoned_close_future_does_not_abandon_cleanup() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let handle = owner.handle();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let registration = handle
        .register(ResourceKind::Registration, 1, || async move {
            started.send(()).unwrap();
            wait.await.unwrap();
            Ok(())
        })
        .unwrap();
    let closing = tokio::spawn(async move { registration.close(WAIT).await });
    ready.await.unwrap();
    closing.abort();
    let _ = closing.await;
    assert_eq!(handle.report().remaining.len(), 1);
    release.send(()).unwrap();
    assert!(owner.close(WAIT).await.is_quiescent());
}

#[tokio::test]
async fn opening_a_handle_reserves_cleanup_before_dispatch_and_survives_lost_delivery() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor
        .owner(OwnerLimits {
            resources: 1,
            retained_bytes: 8,
        })
        .unwrap();
    let h = owner.handle();
    let released = Arc::new(AtomicUsize::new(0));
    let cleanup_count = released.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        h.open(8, move |_| async move {
            started.send(()).unwrap();
            // Opening a remote handle has committed and cannot be interrupted.
            wait.await.unwrap();
            Ok((42u64, move || async move {
                cleanup_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
        })
        .await
    });
    ready.await.unwrap();
    assert!(owner.handle().track(ResourceKind::Task, 0).is_err());
    caller.abort();
    let _ = caller.await;
    let report = owner.close(Duration::ZERO).await;
    assert_eq!(report.remaining.len(), 1);
    assert_eq!(report.remaining[0].bytes, 8);
    release.send(()).unwrap();
    assert!(supervisor.close(WAIT).await[0].is_quiescent());
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn owned_open_rejects_before_effect_and_delivered_handles_are_revocable() {
    let supervisor = CleanupSupervisor::new(2).unwrap();
    let denied = supervisor
        .owner(OwnerLimits {
            resources: 0,
            retained_bytes: 0,
        })
        .unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let seen = effects.clone();
    let result = denied
        .handle()
        .open(0, move |_| async move {
            seen.fetch_add(1, Ordering::SeqCst);
            Ok(((), || async { Ok(()) }))
        })
        .await;
    assert!(result.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let resource = owner
        .handle()
        .open(0, |_| async { Ok((42, || async { Ok(()) })) })
        .await
        .unwrap();
    assert_eq!(resource.with(|v| *v).unwrap(), 42);
    assert!(owner.close(WAIT).await.is_quiescent());
    assert!(resource.with(|v| *v).is_err());
}
