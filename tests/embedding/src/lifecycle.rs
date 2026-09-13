//! Executable host lifecycle: a real loopback HTTP disconnect during a guest's
//! external allocation. The tiny HTTP exchange is a deterministic fixture, not
//! an HTTP server implementation. Run the `cancelled_http` example.
use featherweight_runtime::{
    async_host_store, AssemblyDef, CoreWasmEngine, ExecutionScope, Runtime,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use structfs_core_store::{
    path, DetachedFuture, DetachedReader, DetachedWriter, Error, Path, Record,
};
use structfs_handles::CancelToken;
use structfs_service::{CleanupSupervisor, OwnedResource, OwnerHandle, OwnerLimits};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{oneshot, Semaphore},
};

// A host-owned teardown task is returned as ownership, not as the HTTP
// operation's next future. Joining it is a separate host lifecycle decision.
struct RetainedCleanup(tokio::task::JoinHandle<()>);
impl RetainedCleanup {
    async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.0.await
    }
}

type DemoResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const HTTP_REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
const GUEST: &str = r#"(module
 (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
 (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
 (memory (export "memory") 1)
 (func (export "block_alloc") (param i32) (result i32) (i32.const 4096))
 (data (i32.const 32) "{\"serialization\":\"application/json\"}")
 (data (i32.const 128) "iso/server/requests")
 (data (i32.const 192) "broker/open")
 (data (i32.const 256) "{}")
 (func (export "manifest") (param $ret i32) (result i32)
  (i32.store (local.get $ret) (i32.const 32))
  (i32.store offset=4 (local.get $ret) (i32.const 36)) (i32.const 0))
 (func (export "run") (result i32)
  (drop (call $read (i32.const 128) (i32.const 19) (i32.const 1024)))
  (drop (call $write (i32.const 192) (i32.const 11) (i32.const 256) (i32.const 2) (i32.const 1024)))
  (i32.const 7)))"#;

#[derive(Clone)]
struct Allocation {
    accepted: Arc<Semaphore>,
    reply: Arc<Semaphore>,
    join: Arc<Semaphore>,
    cleanup_started: Arc<Semaphore>,
    live: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
}
impl Allocation {
    fn new() -> Self {
        Self {
            accepted: Arc::new(Semaphore::new(0)),
            reply: Arc::new(Semaphore::new(0)),
            join: Arc::new(Semaphore::new(0)),
            cleanup_started: Arc::new(Semaphore::new(0)),
            live: Arc::default(),
            released: Arc::default(),
        }
    }
}
struct Broker {
    owner: OwnerHandle,
    allocation: Allocation,
    alias: Path,
    fail_cleanup: bool,
    handles: Arc<Mutex<Vec<OwnedResource<Path>>>>,
}
impl DetachedReader for Broker {
    fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
        Box::pin(async { Ok(None) })
    }
}
impl DetachedWriter for Broker {
    fn write_detached(&mut self, to: &Path, _: Record) -> DetachedFuture<Path> {
        if to != &path!("open") {
            return Box::pin(async { Err(Error::permission_denied("only open is exposed")) });
        }
        let owner = self.owner.clone();
        let allocation = self.allocation.clone();
        let alias = self.alias.clone();
        let fail_cleanup = self.fail_cleanup;
        let handles = self.handles.clone();
        Box::pin(async move {
            let handle = owner
                .open(64, move |_request_cancel| async move {
                    allocation.live.fetch_add(1, Ordering::SeqCst);
                    let stop = CancelToken::new();
                    let producer_stop = stop.clone();
                    let producer_state = allocation.clone();
                    let producer = tokio::spawn(async move {
                        producer_stop.cancelled().await;
                        producer_state.join.acquire().await.unwrap().forget();
                        producer_state.live.fetch_sub(1, Ordering::SeqCst);
                    });
                    allocation.accepted.add_permits(1);
                    // The external broker has accepted the effect. A disconnected
                    // caller must not cancel observation of its eventual result.
                    allocation.reply.acquire().await.unwrap().forget();
                    Ok((alias, move || async move {
                        // Cleanup uses host-owned identity, not a guessed path
                        // derived from the public alias, and joins actual work.
                        allocation.cleanup_started.add_permits(1);
                        stop.cancel();
                        producer
                            .await
                            .map_err(|e| Error::store("demo", "join", e.to_string()))?;
                        allocation.released.fetch_add(1, Ordering::SeqCst);
                        if fail_cleanup {
                            Err(Error::store(
                                "demo",
                                "release",
                                "release acknowledgement lost",
                            ))
                        } else {
                            Ok(())
                        }
                    }))
                })
                .await?;
            let result = handle.with(Clone::clone)?;
            handles.lock().unwrap().push(handle);
            Ok(result)
        })
    }
}

/// Run the entire disconnect/late-reply scenario twice with one prepared module,
/// then show a nonzero guest exit. All executors and supervisors outlive cleanup.
pub async fn cancelled_http_demo() -> DemoResult<()> {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536)?;
    let prepared = Arc::new(engine.prepare(GUEST.as_bytes().to_vec()).await?);
    let supervisor = Arc::new(CleanupSupervisor::new(1)?);
    let definition = AssemblyDef::from_str(
        r#"{"assembly":"http","blocks":{"server":"prepared:http"},"public":"server","imports":{"broker":"external allocator"},"wiring":["server:/broker -> $broker"]}"#,
    )?;
    for (round, alias) in [path!("outstanding/1"), path!("aliases/same_resource")]
        .into_iter()
        .enumerate()
    {
        let fail_cleanup = round == 1;
        let allocation = Allocation::new();
        let owner = supervisor.owner(OwnerLimits {
            resources: 4,
            retained_bytes: 256,
        })?;
        let owner_handle = owner.handle();
        let scope = ExecutionScope::new(Duration::from_secs(10));
        let reservation = engine.reserve_session(1)?;
        let mut runtime = Runtime::new().with_execution_scope(scope.clone());
        runtime.register_core_artifact("prepared:http", prepared.in_session(reservation.clone())?);
        let imports = HashMap::from([(
            "broker".into(),
            async_host_store(Broker {
                owner: owner_handle.clone(),
                allocation: allocation.clone(),
                alias,
                fail_cleanup,
                handles: Arc::default(),
            }),
        )]);
        let assembly = runtime.instantiate(&definition, imports, ".".as_ref())?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let (incomplete_tx, incomplete_rx) = oneshot::channel();
        let handler = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = vec![0; HTTP_REQUEST.len()];
            socket.read_exact(&mut header).await.unwrap();
            assert_eq!(header, HTTP_REQUEST);
            let mut byte = [0];
            tokio::select! {
                result = socket.read(&mut byte) => assert_eq!(result.unwrap(), 0),
                result = assembly.read(path!("answer")) => panic!("request ended before disconnect: {result:?}"),
            }
            // HTTP handling ends here. The host retains this teardown task
            // separately; dropping an HTTP response cannot drop its ownership.
            scope.cancel();
            owner.cancel();
            RetainedCleanup(tokio::spawn(async move {
                let shutdown = assembly.shutdown(Duration::from_secs(1)).await;
                assert!(shutdown.complete(), "{shutdown:?}");
                assert_eq!(runtime.registered_blocks(), 0);
                let report = owner.close(Duration::ZERO).await;
                assert!(!report.is_quiescent());
                incomplete_tx.send(report).unwrap();
                // A grace timeout is a report, not permission to release the
                // reservation. Retain everything until cleanup really joins.
                loop {
                    let report = owner.close(Duration::from_secs(1)).await;
                    if report.is_quiescent() {
                        break;
                    }
                    // Failed cleanup stays charged too. The host must observe
                    // and reconcile it; this task must not drop its reservation.
                    if report.failures > 0 {
                        eprintln!("cleanup requires reconciliation: {report:?}");
                    }
                }
                drop(assembly);
                drop(runtime);
                drop(reservation);
            }))
        });
        let mut client = TcpStream::connect(address).await?;
        client.write_all(HTTP_REQUEST).await?;
        allocation.accepted.acquire().await?.forget();
        drop(client); // Actual client disconnect while allocation is outstanding.
        let cleanup = handler.await?;
        let report = incomplete_rx.await?;
        assert!(!report.remaining.is_empty());
        assert!(engine.reserve_session(1).is_err());
        assert!(supervisor.owner(OwnerLimits::default()).is_err());
        // Accepted result arrives after the caller has vanished.
        allocation.reply.add_permits(1);
        allocation.cleanup_started.acquire().await?.forget();
        // Now we know ownership received the late reply, not merely that the
        // broker was permitted to send it. The actual producer is still live.
        assert_eq!(allocation.live.load(Ordering::SeqCst), 1);
        assert!(engine.reserve_session(1).is_err());
        allocation.join.add_permits(1);
        if fail_cleanup {
            let failed = loop {
                let report = owner_handle.close(Duration::from_millis(10)).await;
                if report.failures > 0 {
                    break report;
                }
            };
            assert!(!failed.is_quiescent());
            assert!(engine.reserve_session(1).is_err());
            assert!(supervisor.owner(OwnerLimits::default()).is_err());
            // Explicit external reconciliation: our fixture can inspect the
            // actual resource and confirm release despite a lost acknowledgement.
            // A real host must obtain equally authoritative evidence first.
            assert_eq!(allocation.live.load(Ordering::SeqCst), 0);
            assert_eq!(allocation.released.load(Ordering::SeqCst), 1);
            for resource in failed.remaining {
                assert!(resource.failed);
                owner_handle.acknowledge_failure(resource.id)?;
            }
        }
        cleanup.join().await?;
        assert!(owner_handle.report().is_quiescent());
        assert_eq!(allocation.live.load(Ordering::SeqCst), 0);
        assert_eq!(allocation.released.load(Ordering::SeqCst), 1);
        drop(engine.reserve_session(1)?);
    }
    assert!(supervisor
        .close(Duration::from_secs(1))
        .await
        .iter()
        .all(|r| r.is_quiescent()));
    // Reuse the same prepared guest with an immediately completing broker to
    // observe its nonzero terminal result, rather than infer success from stop.
    let owner_supervisor = CleanupSupervisor::new(1)?;
    let owner = owner_supervisor.owner(OwnerLimits::default())?;
    let allocation = Allocation::new();
    allocation.reply.add_permits(1);
    allocation.join.add_permits(1);
    let reservation = engine.reserve_session(1)?;
    let mut runtime = Runtime::new();
    runtime.register_core_artifact("prepared:http", prepared.in_session(reservation.clone())?);
    let assembly = runtime.instantiate(
        &definition,
        HashMap::from([(
            "broker".into(),
            async_host_store(Broker {
                owner: owner.handle(),
                allocation,
                alias: path!("outstanding/1"),
                fail_cleanup: false,
                handles: Arc::default(),
            }),
        )]),
        ".".as_ref(),
    )?;
    assert!(assembly.read(path!("answer")).await.is_err());
    assembly.wait_public_terminal().await;
    assert_eq!(assembly.public_cell().exit_code(), 7);
    // A declared nonzero exit need not carry a trap diagnostic. Inspect both
    // exit_code and last_error; terminal state alone is not success.
    assert!(assembly.shutdown(Duration::from_secs(1)).await.complete());
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    drop(assembly);
    drop(runtime);
    drop(reservation);
    // The caller's Tokio runtime must remain alive through this point: both
    // the engine ticker and retained cleanup execute on it.
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disconnect_late_reply_alias_and_nonzero_exit() {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            super::cancelled_http_demo(),
        )
        .await
        .unwrap()
        .unwrap();
    }
}
