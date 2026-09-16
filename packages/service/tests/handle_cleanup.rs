use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use structfs_core_store::{
    path, DetachedFuture, DetachedReader, DetachedWriter, Error, Path, Record, Value,
};
use structfs_handles::{CancelToken, HandleCx, HandleProtocol, HandleStore};
use structfs_service::{CleanupSupervisor, OwnerLimits, SupervisedProtocol};
struct Protocol {
    release: Arc<tokio::sync::Notify>,
    alive: Arc<AtomicUsize>,
    fail: bool,
}
struct Handle {
    cancel: CancelToken,
    task: Mutex<Option<tokio::task::JoinHandle<Result<(), Error>>>>,
}
struct Resource(Arc<AtomicUsize>);
impl Drop for Resource {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl HandleProtocol for Protocol {
    type Handle = Handle;
    fn open(&self, cx: HandleCx, _: Value) -> Result<Handle, Error> {
        self.alive.fetch_add(1, Ordering::SeqCst);
        let resource = Resource(self.alive.clone());
        let cancel = cx.cancel.clone();
        let release = self.release.clone();
        let fail = self.fail;
        let task = tokio::spawn(async move {
            let _resource = resource;
            cancel.cancelled().await;
            release.notified().await;
            if fail {
                Err(Error::store("producer", "finish", "injected failure"))
            } else {
                Ok(())
            }
        });
        Ok(Handle {
            cancel: cx.cancel,
            task: Mutex::new(Some(task)),
        })
    }
    fn read(&self, h: Arc<Handle>, _: Path) -> DetachedFuture<Option<Record>> {
        Box::pin(async move {
            h.cancel.cancelled().await;
            Err(Error::cancelled("released"))
        })
    }
    fn write(&self, _: Arc<Handle>, path: Path, _: Record) -> DetachedFuture<Path> {
        Box::pin(async { Ok(path) })
    }
}
async fn join(h: Arc<Handle>) -> Result<(), Error> {
    let task = h.task.lock().unwrap().take().unwrap();
    task.await
        .map_err(|e| Error::store("producer", "join", e.to_string()))?
}
#[tokio::test]
async fn release_waits_for_resources_and_repeated_release_and_abandonment_are_safe() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let alive = Arc::new(AtomicUsize::new(0));
    let mut store = HandleStore::new(SupervisedProtocol::new(
        Protocol {
            release: release.clone(),
            alive: alive.clone(),
            fail: false,
        },
        owner.handle(),
        Duration::from_millis(10),
        join,
    ));
    let handle = store
        .write_detached(&path!(""), Record::parsed(Value::Null))
        .await
        .unwrap();
    let parked = store.read_detached(&handle);
    drop(store.write_detached(&handle, Record::parsed(Value::Null)));
    assert!(parked.await.is_err());
    assert_eq!(store.live_handles(), 0);
    assert!(store.read_detached(&handle).await.unwrap().is_none());
    assert!(store
        .write_detached(&handle, Record::parsed(Value::Null))
        .await
        .is_err());
    assert_eq!(alive.load(Ordering::SeqCst), 1);
    let a = store.write_detached(&handle, Record::parsed(Value::Null));
    let b = store.write_detached(&handle, Record::parsed(Value::Null));
    release.notify_one();
    let (a, b) = tokio::join!(a, b);
    a.unwrap();
    b.unwrap();
    assert_eq!(alive.load(Ordering::SeqCst), 0);
    store
        .write_detached(&handle, Record::parsed(Value::Null))
        .await
        .unwrap();
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
}
#[tokio::test]
async fn final_store_drop_retains_cleanup_and_reports_producer_failure() {
    for fail in [false, true] {
        let supervisor = CleanupSupervisor::new(1).unwrap();
        let owner = supervisor.owner(OwnerLimits::default()).unwrap();
        let release = Arc::new(tokio::sync::Notify::new());
        let alive = Arc::new(AtomicUsize::new(0));
        let mut store = HandleStore::new(SupervisedProtocol::new(
            Protocol {
                release: release.clone(),
                alive: alive.clone(),
                fail,
            },
            owner.handle(),
            Duration::from_millis(10),
            join,
        ));
        store
            .write_detached(&path!(""), Record::parsed(Value::Null))
            .await
            .unwrap();
        drop(store);
        assert!(!owner.close(Duration::ZERO).await.is_quiescent());
        release.notify_one();
        let report = owner.close(Duration::from_millis(50)).await;
        assert_eq!(alive.load(Ordering::SeqCst), 0);
        assert_eq!(report.is_quiescent(), !fail);
        if fail {
            assert_eq!(report.failures, 1);
            for r in report.remaining {
                assert!(r.failed);
                owner.handle().acknowledge_failure(r.id).unwrap();
            }
        }
        assert!(supervisor
            .close(Duration::from_secs(1))
            .await
            .iter()
            .all(|r| r.is_quiescent()));
    }
}
