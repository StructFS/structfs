use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use structfs_core_store::{
    path, DetachedFuture, DetachedReader, DetachedWriter, Error, Path, Record, Value,
};
use structfs_handles::{CancelToken, HandleCx, HandleProtocol, HandleStore};
use structfs_service::{CleanupSupervisor, OwnerLimits, SupervisedProtocol};
#[derive(Default)]
struct Protocol {
    release: Arc<tokio::sync::Notify>,
    alive: Arc<AtomicUsize>,
    fail: bool,
    terminal: Arc<AtomicBool>,
    close_calls: Arc<AtomicUsize>,
    panic_on_close: bool,
}
struct Handle {
    terminal: Arc<AtomicBool>,
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
            terminal: self.terminal.clone(),
            cancel: cx.cancel,
            task: Mutex::new(Some(task)),
        })
    }
    fn close(&self, _: Arc<Handle>) {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_on_close, "injected shutdown-hook panic");
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
    let result = task
        .await
        .map_err(|e| Error::store("producer", "join", e.to_string()));
    h.terminal.store(true, Ordering::SeqCst);
    result?
}
async fn assert_pending<F: std::future::Future + Unpin>(future: &mut F) {
    std::future::poll_fn(|cx| {
        assert!(std::pin::Pin::new(&mut *future).poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}
#[tokio::test]
async fn release_waits_for_resources_and_repeated_release_and_abandonment_are_safe() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let alive = Arc::new(AtomicUsize::new(0));
    let terminal = Arc::new(AtomicBool::new(false));
    let close_calls = Arc::new(AtomicUsize::new(0));
    let mut store = HandleStore::new(SupervisedProtocol::new(
        Protocol {
            release: release.clone(),
            alive: alive.clone(),
            fail: false,
            terminal: terminal.clone(),
            close_calls: close_calls.clone(),
            ..Protocol::default()
        },
        owner.handle(),
        Duration::from_millis(10),
        join,
    ));
    let handle = store
        .write_detached(&path!(""), Record::parsed(Value::Null))
        .await
        .unwrap();
    let mut parked = store.read_detached(&handle);
    assert_pending(&mut parked).await;
    let mut abandoned = store.write_detached(&handle, Record::parsed(Value::Null));
    assert_pending(&mut abandoned).await;
    drop(abandoned);
    assert!(!terminal.load(Ordering::SeqCst));
    assert!(parked.await.is_err());
    assert_eq!(store.live_handles(), 0);
    assert!(store.read_detached(&handle).await.unwrap().is_none());
    assert!(store
        .write_detached(&handle, Record::parsed(Value::Null))
        .await
        .is_err());
    assert_eq!(alive.load(Ordering::SeqCst), 1);
    let a = store.write_detached(&handle, Record::parsed(Value::Null));
    let mut alias = store.clone();
    let b = alias.write_detached(&handle, Record::parsed(Value::Null));
    release.notify_one();
    let (a, b) = tokio::join!(a, b);
    a.unwrap();
    b.unwrap();
    assert_eq!(alive.load(Ordering::SeqCst), 0);
    assert!(terminal.load(Ordering::SeqCst));
    assert_eq!(close_calls.load(Ordering::SeqCst), 1);
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
                ..Protocol::default()
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

#[tokio::test]
async fn shutdown_hook_panic_still_joins_and_publishes_terminal_state() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let terminal = Arc::new(AtomicBool::new(false));
    let release = Arc::new(tokio::sync::Notify::new());
    let alive = Arc::new(AtomicUsize::new(0));
    let mut store = HandleStore::new(SupervisedProtocol::new(
        Protocol {
            terminal: terminal.clone(),
            release: release.clone(),
            alive: alive.clone(),
            panic_on_close: true,
            ..Protocol::default()
        },
        owner.handle(),
        Duration::from_millis(50),
        join,
    ));
    let handle = store
        .write_detached(&path!(""), Record::parsed(Value::Null))
        .await
        .unwrap();
    let mut closing = store.write_detached(&handle, Record::parsed(Value::Null));
    assert_pending(&mut closing).await;
    assert!(!terminal.load(Ordering::SeqCst));
    release.notify_one();
    assert!(closing.await.is_err());
    assert!(terminal.load(Ordering::SeqCst));
    assert_eq!(alive.load(Ordering::SeqCst), 0);
    let report = owner.handle().report();
    assert_eq!(report.failures, 1);
    for remaining in report.remaining {
        assert!(remaining.failed);
        owner.handle().acknowledge_failure(remaining.id).unwrap();
    }
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
}

#[tokio::test]
async fn owner_close_during_open_does_not_block_executor_and_joins_late_handle() {
    struct Opening {
        protocol: Protocol,
        entered: Arc<tokio::sync::Notify>,
        proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl HandleProtocol for Opening {
        type Handle = Handle;
        fn open(&self, cx: HandleCx, request: Value) -> Result<Handle, Error> {
            self.entered.notify_one();
            self.proceed
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .unwrap();
            self.protocol.open(cx, request)
        }
        fn read(&self, h: Arc<Handle>, p: Path) -> DetachedFuture<Option<Record>> {
            self.protocol.read(h, p)
        }
        fn write(&self, h: Arc<Handle>, p: Path, r: Record) -> DetachedFuture<Path> {
            self.protocol.write(h, p, r)
        }
        fn close(&self, h: Arc<Handle>) {
            self.protocol.close(h);
        }
    }
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let terminal = Arc::new(AtomicBool::new(false));
    let release = Arc::new(tokio::sync::Notify::new());
    let (proceed, receiver) = std::sync::mpsc::channel();
    let protocol = SupervisedProtocol::new(
        Opening {
            protocol: Protocol {
                terminal: terminal.clone(),
                release: release.clone(),
                ..Protocol::default()
            },
            entered: entered.clone(),
            proceed: Mutex::new(receiver),
        },
        owner.handle(),
        Duration::from_secs(1),
        join,
    );
    let opening = tokio::task::spawn_blocking(move || {
        let mut store = HandleStore::new(protocol);
        let delivery = store.write_detached(&path!(""), Record::parsed(Value::Null));
        (store, delivery)
    });
    entered.notified().await;
    let start = std::time::Instant::now();
    let report = owner.close(Duration::from_millis(20)).await;
    // The old blocking handoff stalls this current-thread executor until the
    // opening watchdog expires. The async handoff leaves timers runnable.
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!report.is_quiescent());
    proceed.send(()).unwrap();
    let (store, delivery) = opening.await.unwrap();
    drop(delivery); // caller abandons the late allocation result
    drop(store);
    assert!(!terminal.load(Ordering::SeqCst));
    release.notify_one();
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    assert!(terminal.load(Ordering::SeqCst));
}
