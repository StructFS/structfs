use featherweight_runtime::{
    service_host_store, AssemblyDef, CallBudget, CallLimits, CoreWasmEngine, Runtime,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use structfs_core_store::{DetachedFuture, Path, Record, Value};
use structfs_service::{CallContext, Operation, Response, Service};
struct Capture {
    seen: Mutex<Vec<(Path, Option<Value>)>>,
    redirect: bool,
}
impl Service for Capture {
    fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
        let response = match op {
            Operation::Read(p) => {
                self.seen.lock().unwrap().push((p, None));
                Response::Read(Some(Record::parsed(Value::Unsigned(u64::MAX))))
            }
            Operation::Write(p, r) => {
                self.seen
                    .lock()
                    .unwrap()
                    .push((p.clone(), r.as_value().cloned()));
                Response::Written(if self.redirect {
                    Path::parse("outside").unwrap()
                } else {
                    p
                })
            }
        };
        Box::pin(async move {
            c.ensure_active()?;
            Ok(response)
        })
    }
}
// A fresh guest performs a write and a read through its wired native import.
// With a single call slot, double charging would reject either operation.
fn guest() -> Vec<u8> {
    let manifest = r#"{"serialization":"application/vnd.structfs.value+json;version=1"}"#;
    let payload = r#"["structfs-value",1,["int","18446744073709551615"]]"#;
    let escape = |s: &str| s.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    format!(r#"(module
      (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
      (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "block_alloc") (param i32) (result i32) (i32.const 4096))
      (data (i32.const 32) "{}")
      (data (i32.const 256) "svc/item")
      (data (i32.const 512) "{}")
      (func (export "manifest") (param $ret i32) (result i32)
        (i32.store (local.get $ret) (i32.const 32))
        (i32.store offset=4 (local.get $ret) (i32.const {})) (i32.const 0))
      (func (export "run") (result i32)
        (if (call $write (i32.const 256) (i32.const 8) (i32.const 512) (i32.const {}) (i32.const 1024)) (then unreachable))
        (if (call $read (i32.const 256) (i32.const 8) (i32.const 1024)) (then unreachable))
        (i32.const 0)))"#,escape(manifest),escape(payload),manifest.len(),payload.len()).into_bytes()
}
#[tokio::test]
async fn guest_imports_use_shared_routing_and_admit_each_call_once() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(engine.prepare(guest()).await.unwrap());
    let budget = CallBudget::new(CallLimits {
        calls: 1,
        calls_per_block: 1,
        ..CallLimits::default()
    });
    let mut runtime = Runtime::new().with_call_budget(budget.clone());
    runtime.register_core_artifact("service-test", code);
    let definition=AssemblyDef::from_str(r#"{"assembly":"service-test","blocks":{"guest":"service-test"},"public":"guest","imports":{"store":"native"},"wiring":["guest:/svc -> $store"]}"#).unwrap();
    let capture = Arc::new(Capture {
        seen: Mutex::new(vec![]),
        redirect: false,
    });
    let assembly = runtime
        .instantiate(
            &definition,
            HashMap::from([("store".into(), service_host_store(capture.clone()))]),
            ".".as_ref(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), assembly.wait_public_terminal())
        .await
        .unwrap();
    assert_eq!(
        assembly.public_cell().state(),
        featherweight_runtime::BlockState::Stopped,
        "{:?}",
        assembly.public_cell().last_error()
    );
    assert_eq!(budget.metrics().admitted, 2);
    assert_eq!(budget.metrics().rejected, 0);
    assert_eq!(budget.usage().calls, 0);
    let seen = capture.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0],
        (
            Path::parse("item").unwrap(),
            Some(Value::Unsigned(u64::MAX))
        )
    );
    drop(seen);
    assembly.shutdown(Duration::ZERO).await;
}

#[tokio::test]
async fn direct_provider_admission_can_reject_before_dispatch() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(engine.prepare(guest()).await.unwrap());
    let budget = CallBudget::new(CallLimits {
        calls: 0,
        ..CallLimits::default()
    });
    let mut runtime = Runtime::new().with_call_budget(budget.clone());
    runtime.register_core_artifact("service-test", code);
    let definition=AssemblyDef::from_str(r#"{"assembly":"service-test","blocks":{"guest":"service-test"},"public":"guest","imports":{"store":"native"},"wiring":["guest:/svc -> $store"]}"#).unwrap();
    let capture = Arc::new(Capture {
        seen: Mutex::new(vec![]),
        redirect: false,
    });
    let assembly = runtime
        .instantiate(
            &definition,
            HashMap::from([("store".into(), service_host_store(capture.clone()))]),
            ".".as_ref(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), assembly.wait_public_terminal())
        .await
        .unwrap();
    assert_eq!(
        assembly.public_cell().state(),
        featherweight_runtime::BlockState::Failed
    );
    assert!(capture.seen.lock().unwrap().is_empty());
    assert_eq!(budget.metrics().rejected, 1);
    assert_eq!(budget.usage().calls, 0);
    assembly.shutdown(Duration::ZERO).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mailbox_target_reuses_router_reservation() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(engine.prepare(guest()).await.unwrap());
    let budget = CallBudget::new(CallLimits {
        calls: 1,
        calls_per_block: 1,
        ..CallLimits::default()
    });
    let mut runtime = Runtime::new().with_call_budget(budget.clone());
    featherweight_runtime::register_builtins(&mut runtime);
    runtime.register_core_artifact("service-test", code);
    let definition=AssemblyDef::from_str(r#"{"assembly":"service-test","blocks":{"guest":"service-test","store":"builtin:kv"},"public":"guest","wiring":["guest:/svc -> store"]}"#).unwrap();
    let assembly = runtime
        .instantiate(&definition, HashMap::new(), ".".as_ref())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), assembly.wait_public_terminal())
        .await
        .unwrap();
    assert_eq!(
        assembly.public_cell().state(),
        featherweight_runtime::BlockState::Stopped,
        "{:?}",
        assembly.public_cell().last_error()
    );
    assert_eq!(budget.metrics().admitted, 2);
    assert_eq!(budget.metrics().rejected, 0);
    assert_eq!(budget.usage().calls, 0);
    assembly.shutdown(Duration::from_millis(100)).await;
}

struct BackgroundProvider {
    started: std::sync::atomic::AtomicBool,
    release: Arc<tokio::sync::Notify>,
}
impl Service for BackgroundProvider {
    fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
        if !self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let release = self.release.clone();
            context
                .owner()
                .unwrap()
                .clone()
                .spawn(move |_| async move {
                    // Represents a committed effect which cannot be stopped simply
                    // because the guest or request has finished.
                    let _context = context;
                    release.notified().await;
                    Ok(())
                })
                .unwrap();
        }
        Box::pin(async move {
            Ok(match op {
                Operation::Read(_) => Response::Read(Some(Record::parsed(Value::Null))),
                Operation::Write(p, _) => Response::Written(p),
            })
        })
    }
}
#[tokio::test]
async fn shutdown_reports_owned_provider_work_after_guest_execution_has_joined() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(engine.prepare(guest()).await.unwrap());
    let budget = CallBudget::new(CallLimits::default());
    let mut runtime = Runtime::new().with_call_budget(budget.clone());
    let supervisor = runtime.cleanup_supervisor();
    runtime.register_core_artifact("owned-test", code);
    let definition = AssemblyDef::from_str(r#"{"assembly":"owned-test","blocks":{"guest":"owned-test"},"public":"guest","imports":{"store":"native"},"wiring":["guest:/svc -> $store"]}"#).unwrap();
    let release = Arc::new(tokio::sync::Notify::new());
    let assembly = runtime
        .instantiate(
            &definition,
            HashMap::from([(
                "store".into(),
                service_host_store(Arc::new(BackgroundProvider {
                    started: std::sync::atomic::AtomicBool::new(false),
                    release: release.clone(),
                })),
            )]),
            ".".as_ref(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), assembly.wait_public_terminal())
        .await
        .unwrap();
    let report = assembly.shutdown(Duration::ZERO).await;
    assert!(report.remaining.is_empty());
    assert!(!report.complete());
    assert_eq!(report.providers[0].remaining.len(), 2);
    assert_eq!(budget.usage().calls, 1);
    // The host can abandon the instance and still join through its retained
    // supervisor; the noninterruptible operation stays charged in the interim.
    drop(assembly);
    drop(runtime);
    release.notify_one();
    let reports = supervisor.close(Duration::from_secs(2)).await;
    assert!(reports.iter().all(|r| r.is_quiescent()));
    assert_eq!(budget.usage().calls, 0);
}
