//! Fresh core guests over bounded fake upstream/response capabilities.
#[cfg(test)]
mod tests {
    use featherweight_runtime::{
        async_host_store, service_host_store, AssemblyDef, BlockState, CoreWasmEngine, Metering,
        Runtime,
    };
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    use structfs_core_store::{DetachedFuture, Error, Record, Value};
    use structfs_handles::{CancelToken, DuplexStream};
    use structfs_service::*;
    struct Upstream {
        payload: Vec<u8>,
        released: Arc<AtomicUsize>,
        opened: AtomicBool,
    }
    impl Service for Upstream {
        fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
            let payload = self.payload.clone();
            let released = self.released.clone();
            let owner = c.owner().unwrap().clone();
            // Bound the fake provider to one opened handle per fresh execution.
            if self.opened.swap(true, Ordering::SeqCst) {
                return Box::pin(async { Err(Error::overloaded("upstream handles")) });
            }
            let Operation::Read(_) = op else {
                return Box::pin(async { Err(Error::permission_denied("upstream read")) });
            };
            // Registration is installed before the result is exposed. Keep it in the
            // provider even if the guest traps without issuing any release writes.
            let registration = owner
                .register(ResourceKind::Registration, 8, move || async move {
                    released.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap();
            // A registration owned by the instance outlives this returned future.
            // Store its guard using a dedicated retained field below via a wrapper lease.
            let hold = Lease::new(registration);
            let result = if payload.len() > 8 {
                Err(Error::resource_limit("response cap"))
            } else {
                Ok(Response::Read(Some(Record::parsed(Value::Bytes(payload)))))
            };
            // Retaining a context-independent reservation is explicit, not a detached task.
            owner
                .spawn(move |cancel| async move {
                    let _hold = hold;
                    cancel.cancelled().await;
                    Ok(())
                })
                .unwrap();
            Box::pin(async move {
                let _context = c;
                result
            })
        }
    }
    fn guest(tail: &str, twice: bool) -> Vec<u8> {
        let second = if twice {
            "(if (call $write (i32.const 300) (i32.const 11) (i32.const 8192) (local.get $len) (i32.const 1032)) (then unreachable))"
        } else {
            ""
        };
        format!(r#"(module
  (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
  (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "block_alloc") (param i32) (result i32) (i32.const 32768))
  (data (i32.const 32) "{{\22serialization\22:\22application/cbor\22}}")
  (data (i32.const 256) "upstream/open") (data (i32.const 300) "response/tx")
  (data (i32.const 350) "response/shutdown") (data (i32.const 400) "\f6")
  (func (export "manifest") (param $r i32) (result i32)
   (i32.store (local.get $r) (i32.const 32)) (i32.store offset=4 (local.get $r) (i32.const 36)) (i32.const 0))
  (func (export "run") (result i32) (local $len i32)
   (if (call $read (i32.const 256) (i32.const 13) (i32.const 1024)) (then unreachable))
   (local.set $len (i32.load (i32.const 1028))) (memory.copy (i32.const 8192) (i32.load (i32.const 1024)) (local.get $len))
   {tail}
   (if (call $write (i32.const 300) (i32.const 11) (i32.const 8192) (local.get $len) (i32.const 1032)) (then unreachable))
   {second}
   (if (call $write (i32.const 350) (i32.const 17) (i32.const 400) (i32.const 1) (i32.const 1032)) (then unreachable))
   (i32.const 0)))"#).into_bytes()
    }
    fn definition() -> AssemblyDef {
        AssemblyDef::from_str(r#"{"assembly":"gateway","imports":{"upstream":"fake","response":"bounded stream"},"blocks":{"request":"gateway"},"public":"request","wiring":["request:/upstream -> $upstream","request:/response -> $response"]}"#).unwrap()
    }
    #[tokio::test]
    async fn prepared_artifact_fresh_requests_backpressure_eof_and_cleanup() {
        let engine = CoreWasmEngine::with_limits(2, 2, 65536).unwrap();
        let prepared = Arc::new(engine.prepare(guest("", true)).await.unwrap());
        for _ in 0..2 {
            let start = Instant::now();
            let budget = CallBudget::new(CallLimits {
                calls: 1,
                calls_per_block: 1,
                ..Default::default()
            });
            let mut runtime = Runtime::new().with_call_budget(budget.clone());
            runtime.register_core_artifact("gateway", prepared.clone());
            let released = Arc::new(AtomicUsize::new(0));
            let upstream = Arc::new(Upstream {
                payload: vec![0, 255, 128, 10],
                released: released.clone(),
                opened: AtomicBool::new(false),
            });
            let (output, consumer) = DuplexStream::pair(4).unwrap();
            let instance = runtime
                .instantiate(
                    &definition(),
                    HashMap::from([
                        ("upstream".into(), service_host_store(upstream)),
                        ("response".into(), async_host_store(output.store())),
                    ]),
                    ".".as_ref(),
                )
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while consumer.readiness().readable_bytes != 4 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(matches!(
                instance.public_cell().state(),
                BlockState::Running | BlockState::Starting
            ));
            assert_eq!(
                consumer.read(4, &CancelToken::new()).await.unwrap(),
                vec![0, 255, 128, 10]
            );
            assert_eq!(
                consumer.read(4, &CancelToken::new()).await.unwrap(),
                vec![0, 255, 128, 10]
            );
            assert!(consumer
                .read(4, &CancelToken::new())
                .await
                .unwrap()
                .is_empty());
            instance.wait_public_terminal().await;
            assert!(instance.shutdown(Duration::from_secs(1)).await.complete());
            assert_eq!(released.load(Ordering::SeqCst), 1);
            assert_eq!(budget.usage().calls, 0);
            assert!(budget.metrics().admitted >= 4);
            let usage = instance.public_cell().usage.snapshot();
            assert_eq!(usage.peak_linear_memory_bytes, Some(65536));
            eprintln!(
                "gateway workload: {} us, {:?} peak guest bytes",
                start.elapsed().as_micros(),
                usage.peak_linear_memory_bytes
            );
        }
    }
    #[tokio::test]
    async fn trap_fuel_response_overflow_and_disconnect_release_without_guest_cleanup() {
        for mode in ["trap", "fuel", "oversize", "disconnect"] {
            let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
            let tail = match mode {
                "trap" => "unreachable",
                "fuel" => "(loop $spin (br $spin))",
                _ => "",
            };
            let code = Arc::new(engine.prepare(guest(tail, false)).await.unwrap());
            let budget = CallBudget::new(CallLimits::default());
            let mut runtime = Runtime::new()
                .with_call_budget(budget.clone())
                .with_metering(Metering {
                    fuel: Some(100000),
                    ..Default::default()
                });
            runtime.register_core_artifact("gateway", code);
            let released = Arc::new(AtomicUsize::new(0));
            let upstream = Arc::new(Upstream {
                payload: vec![1; if mode == "oversize" { 9 } else { 4 }],
                released: released.clone(),
                opened: AtomicBool::new(false),
            });
            let (output, consumer) = DuplexStream::pair(4).unwrap();
            if mode == "disconnect" {
                consumer.release();
            }
            let instance = runtime
                .instantiate(
                    &definition(),
                    HashMap::from([
                        ("upstream".into(), service_host_store(upstream)),
                        ("response".into(), async_host_store(output.store())),
                    ]),
                    ".".as_ref(),
                )
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), instance.wait_public_terminal())
                .await
                .unwrap();
            assert_eq!(instance.public_cell().state(), BlockState::Failed);
            assert!(instance.shutdown(Duration::from_secs(1)).await.complete());
            assert_eq!(released.load(Ordering::SeqCst), 1);
            assert_eq!(budget.usage().calls, 0);
        }
    }
    #[tokio::test]
    async fn disconnect_during_open_keeps_noncooperative_work_charged_until_joined() {
        let supervisor = CleanupSupervisor::new(1).unwrap();
        let owner = supervisor.owner(Default::default()).unwrap();
        let h = owner.handle();
        let budget = CallBudget::<String>::new(CallLimits {
            calls: 1,
            ..Default::default()
        });
        let lease = budget.acquire_bytes(&"upstream".into(), 4).unwrap();
        assert!(budget.acquire_bytes(&"peer".into(), 4).is_err());
        let (start, started) = tokio::sync::oneshot::channel();
        let (release, waiting) = tokio::sync::oneshot::channel();
        let cleaned = Arc::new(AtomicUsize::new(0));
        let cleanup = cleaned.clone();
        let call = tokio::spawn(async move {
            h.open(4, move |_| async move {
                let _lease = lease;
                start.send(()).unwrap();
                waiting.await.unwrap();
                Ok(((), move || async move {
                    cleanup.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }))
            })
            .await
        });
        started.await.unwrap();
        call.abort();
        let _ = call.await;
        assert!(!owner.close(Duration::from_millis(1)).await.is_quiescent());
        assert_eq!(budget.usage().calls, 1);
        release.send(()).unwrap();
        assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
        assert_eq!(budget.usage().calls, 0);
        assert_eq!(cleaned.load(Ordering::SeqCst), 1);
        let (a, _b) = DuplexStream::pair(4).unwrap();
        assert!(a.write(&[0; 5], &CancelToken::new()).await.is_err());
    }
    struct PendingRead(Arc<AtomicUsize>);
    impl Service for PendingRead {
        fn call(&self, c: CallContext, _: Operation) -> DetachedFuture<Response> {
            let cleaned = self.0.clone();
            Box::pin(async move {
                struct Guard(Arc<AtomicUsize>);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.0.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let _guard = Guard(cleaned);
                let _context = c;
                std::future::pending().await
            })
        }
    }
    #[tokio::test]
    async fn request_timeout_and_disconnect_before_open_do_not_leak_admission() {
        let supervisor = CleanupSupervisor::new(1).unwrap();
        let owner = supervisor.owner(Default::default()).unwrap();
        let budget = CallBudget::<String>::new(CallLimits::default());
        let cleaned = Arc::new(AtomicUsize::new(0));
        let router = Router::new(vec![Mount::new(
            structfs_core_store::path!(""),
            structfs_core_store::path!(""),
            Arc::new(PendingRead(cleaned.clone())),
            Arc::new(BudgetAdmission {
                budget: budget.clone(),
                key: "upstream".into(),
            }),
        )])
        .unwrap();
        let client = router.client().owned_by(&owner.handle()).with_context(
            CallContext::default()
                .with_timeout(Duration::from_millis(5))
                .unwrap(),
        );
        assert!(matches!(
            client.read(&structfs_core_store::path!("open")).await,
            Err(Error::DeadlineExceeded { .. })
        ));
        assert_eq!(cleaned.load(Ordering::SeqCst), 1);
        assert_eq!(budget.usage().calls, 0);
        owner.cancel();
        let started = Arc::new(AtomicUsize::new(0));
        let count = started.clone();
        assert!(owner
            .handle()
            .open(1, move |_| async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(((), || async { Ok(()) }))
            })
            .await
            .is_err());
        assert_eq!(started.load(Ordering::SeqCst), 0);
        assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    }
}
