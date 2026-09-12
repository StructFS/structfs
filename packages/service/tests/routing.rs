use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use structfs_core_store::{
    path, DetachedFuture, DetachedReader, DetachedWriter, Error, MemoryStore, Path, Reader, Record,
    Value, Writer,
};
use structfs_service::*;
fn mount(service: Arc<dyn Service>, budget: Arc<CallBudget>, prefix: Path, base: Path) -> Mount {
    Mount::new(
        prefix,
        base,
        service,
        Arc::new(BudgetAdmission {
            budget,
            key: "provider".into(),
        }),
    )
}
fn budget(calls: usize) -> Arc<CallBudget> {
    CallBudget::new(CallLimits {
        calls,
        calls_per_block: calls,
        ..CallLimits::default()
    })
}
struct Echo {
    redirect: Option<Path>,
    seen: Mutex<Vec<(u64, Path, CancelToken)>>,
}
impl Service for Echo {
    fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
        self.seen
            .lock()
            .unwrap()
            .push((c.request_id, op.path().clone(), c.cancellation.clone()));
        let redirect = self.redirect.clone();
        Box::pin(async move {
            match op {
                Operation::Read(p) => Ok(Response::Read(Some(Record::parsed(Value::String(
                    p.to_string(),
                ))))),
                Operation::Write(p, _) => Ok(Response::Written(redirect.unwrap_or(p))),
            }
        })
    }
}
#[tokio::test]
async fn routing_scopes_and_context_cannot_escape() {
    let b = budget(8);
    let echo = Arc::new(Echo {
        redirect: None,
        seen: Mutex::new(vec![]),
    });
    let r = Router::new(vec![mount(
        echo.clone(),
        b.clone(),
        path!("api"),
        path!("tenant/private"),
    )])
    .unwrap();
    let cx = CallContext::default();
    let id = cx.request_id;
    let client = r
        .client()
        .scoped(&path!("api"), Permissions::READ_WRITE)
        .with_context(cx);
    assert_eq!(
        client
            .read(&path!("item"))
            .await
            .unwrap()
            .unwrap()
            .as_value(),
        Some(&Value::String("tenant/private/item".into()))
    );
    assert_eq!(
        client
            .write(&path!("item"), Record::parsed(Value::Null))
            .await
            .unwrap(),
        path!("item")
    );
    assert_eq!(echo.seen.lock().unwrap()[0].0, id);
    assert!(echo.seen.lock().unwrap()[0].2.is_cancelled());
    let denied = client
        .scoped(&path!(""), Permissions::READ_ONLY)
        .scoped(&path!(""), Permissions::READ_WRITE);
    assert!(matches!(
        denied
            .write(&path!("item"), Record::parsed(Value::Null))
            .await,
        Err(Error::PermissionDenied { .. })
    ));
    assert!(matches!(
        r.client().read(&path!("api_other/item")).await,
        Err(Error::PermissionDenied { .. })
    ));
    assert_eq!(b.metrics().admitted, 2);
    assert_eq!(b.usage().calls, 0);
    let outside = Arc::new(Echo {
        redirect: Some(path!("elsewhere")),
        seen: Mutex::new(vec![]),
    });
    let client = Router::new(vec![mount(
        outside,
        b.clone(),
        path!("api"),
        path!("tenant"),
    )])
    .unwrap()
    .client();
    assert!(matches!(
        client
            .write(&path!("api/item"), Record::parsed(Value::Null))
            .await,
        Err(Error::PermissionDenied { .. })
    ));
    let sibling = Arc::new(Echo {
        redirect: Some(path!("tenant/sibling")),
        seen: Mutex::new(vec![]),
    });
    let client = Router::new(vec![mount(sibling, b, path!("api"), path!("tenant"))])
        .unwrap()
        .client()
        .scoped(&path!("api/child"), Permissions::READ_WRITE);
    assert!(matches!(
        client
            .write(&path!("item"), Record::parsed(Value::Null))
            .await,
        Err(Error::PermissionDenied { .. })
    ));
}
#[tokio::test]
async fn longest_prefix_denial_does_not_fall_back() {
    let b = budget(4);
    let echo = Arc::new(Echo {
        redirect: None,
        seen: Mutex::new(vec![]),
    });
    let root = mount(echo.clone(), b.clone(), path!(""), path!("root"));
    let mut nested = mount(echo.clone(), b.clone(), path!("sealed"), path!("nested"));
    nested.permissions = Permissions::READ_ONLY;
    let client = Router::new(vec![root, nested]).unwrap().client();
    assert!(matches!(
        client
            .write(&path!("sealed/item"), Record::parsed(Value::Null))
            .await,
        Err(Error::PermissionDenied { .. })
    ));
    assert_eq!(
        client
            .read(&path!("sealed/item"))
            .await
            .unwrap()
            .unwrap()
            .as_value(),
        Some(&Value::from("nested/item"))
    );
    assert!(Router::new(vec![
        mount(echo.clone(), b.clone(), path!("a"), path!("")),
        mount(echo, b, path!("a"), path!(""))
    ])
    .is_err());
}
struct Park {
    gate: Arc<structfs_handles::Gate>,
    ready: Arc<AtomicUsize>,
}
impl DetachedReader for Park {
    fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
        let gate = self.gate.clone();
        let ready = self.ready.clone();
        Box::pin(async move {
            ready.fetch_add(1, Ordering::SeqCst);
            gate.wait_until(|| (ready.load(Ordering::SeqCst) > 1).then_some(()))
                .await;
            Ok(Some(Record::parsed(Value::Bool(true))))
        })
    }
}
impl DetachedWriter for Park {
    fn write_detached(&mut self, p: &Path, _: Record) -> DetachedFuture<Path> {
        let gate = self.gate.clone();
        let ready = self.ready.clone();
        let p = p.clone();
        Box::pin(async move {
            ready.fetch_add(1, Ordering::SeqCst);
            gate.notify();
            Ok(p)
        })
    }
}
#[tokio::test(flavor = "current_thread")]
async fn detached_dispatch_releases_lock_and_clients_are_concurrent() {
    let ready = Arc::new(AtomicUsize::new(0));
    let b = budget(2);
    let p = Arc::new(DetachedProvider::new(Park {
        gate: Arc::new(structfs_handles::Gate::new()),
        ready: ready.clone(),
    }));
    let client = Router::new(vec![mount(p, b.clone(), path!(""), path!(""))])
        .unwrap()
        .client();
    let reader = client.clone();
    let task = tokio::spawn(async move { reader.read(&path!("wait")).await });
    while ready.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(b.usage().calls, 1);
    tokio::time::timeout(
        Duration::from_secs(1),
        client.write(&path!("release"), Record::parsed(Value::Null)),
    )
    .await
    .unwrap()
    .unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(b.metrics().admitted, 2);
    assert_eq!(b.usage().calls, 0);
}
struct Pending;
impl Service for Pending {
    fn call(&self, _: CallContext, _: Operation) -> DetachedFuture<Response> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn deadlines_cancellation_admission_and_abandonment() {
    let root = budget(1);
    let child = root.child(CallLimits::default());
    let client = Router::new(vec![mount(
        Arc::new(Pending),
        child.clone(),
        path!(""),
        path!(""),
    )])
    .unwrap()
    .client();
    let context = CallContext::default();
    let token = context.cancellation.clone();
    let c = client.with_context(context);
    let task = tokio::spawn(async move { c.read(&path!("a")).await });
    while root.usage().calls == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(child.usage().calls, 1); // Even a provider that ignores context stays charged.
    assert!(matches!(
        client.read(&path!("b")).await,
        Err(Error::Overloaded { .. })
    ));
    token.cancel();
    assert!(task.await.unwrap().unwrap_err().is_cancelled());
    assert_eq!(root.usage().calls, 0);
    let c = client.with_context(
        CallContext::default()
            .with_timeout(Duration::from_millis(1))
            .unwrap(),
    );
    assert!(matches!(
        c.read(&path!("a")).await,
        Err(Error::DeadlineExceeded { .. })
    ));
    assert_eq!(root.usage().calls, 0);
    let c = client.clone();
    let task = tokio::spawn(async move { c.read(&path!("a")).await });
    while root.usage().calls == 0 {
        tokio::task::yield_now().await;
    }
    task.abort();
    let _ = task.await;
    assert_eq!(root.usage().calls, 0);
    let context = CallContext::default();
    context.cancellation.cancel();
    let admitted = root.metrics().admitted;
    assert!(client
        .with_context(context)
        .read(&path!("a"))
        .await
        .unwrap_err()
        .is_cancelled());
    assert_eq!(root.metrics().admitted, admitted);
    root.set_limits(CallLimits {
        calls: 0,
        ..CallLimits::default()
    });
    assert!(matches!(
        client.read(&path!("a")).await,
        Err(Error::Overloaded { .. })
    ));
}
struct Blocking {
    entered: Arc<AtomicUsize>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
impl Reader for Blocking {
    fn read(&mut self, _: &Path) -> Result<Option<Record>, Error> {
        self.entered.store(1, Ordering::SeqCst);
        self.release.lock().unwrap().recv().unwrap();
        Ok(None)
    }
}
impl Writer for Blocking {
    fn write(&mut self, p: &Path, _: Record) -> Result<Path, Error> {
        Ok(p.clone())
    }
}
#[tokio::test]
async fn cancelled_blocking_work_keeps_charge_until_joined() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let b = budget(1);
    let entered = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    let provider = Arc::new(BlockingStore::new(Blocking {
        entered: entered.clone(),
        release: Mutex::new(rx),
    }));
    let client = Router::new(vec![mount(
        owner.handle().service(provider.clone()),
        b.clone(),
        path!(""),
        path!(""),
    )])
    .unwrap()
    .client();
    let context = CallContext::default();
    let cancellation = context.cancellation.clone();
    let c = client.with_context(context);
    let task = tokio::spawn(async move { c.read(&path!("a")).await });
    while entered.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();
    assert!(task.await.unwrap().unwrap_err().is_cancelled());
    assert_eq!(b.usage().calls, 1);
    assert_eq!(provider.active(), 1);
    assert!(matches!(
        client.read(&path!("b")).await,
        Err(Error::Overloaded { .. })
    ));
    let closing = provider.clone();
    let joined = tokio::spawn(async move { closing.close().await });
    tokio::task::yield_now().await;
    assert!(!joined.is_finished());
    assert!(!owner.close(Duration::ZERO).await.is_quiescent());
    tx.send(()).unwrap();
    joined.await.unwrap();
    assert_eq!(b.usage().calls, 0);
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    assert!(client.read(&path!("a")).await.unwrap_err().is_cancelled());
}
#[tokio::test]
async fn sync_adapter_and_raw_payload_accounting() {
    let b = budget(2);
    let client = Router::new(vec![mount(
        Arc::new(ImmediateStore::new(MemoryStore::new())),
        b.clone(),
        path!(""),
        path!(""),
    )])
    .unwrap()
    .client();
    client
        .write(&path!("a"), Record::parsed(Value::Unsigned(u64::MAX)))
        .await
        .unwrap();
    assert_eq!(
        client.read(&path!("a")).await.unwrap().unwrap().as_value(),
        Some(&Value::Unsigned(u64::MAX))
    );
    b.set_limits(CallLimits {
        bytes: 4,
        ..CallLimits::default()
    });
    assert!(matches!(
        client
            .write(
                &path!("a"),
                Record::raw(vec![0; 32], structfs_core_store::Format::OCTET_STREAM)
            )
            .await,
        Err(Error::Overloaded { .. })
    ));
}
