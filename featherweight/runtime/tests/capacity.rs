//! Fresh-session concurrency proof. Scale manually with FW_CAPACITY=10000.
use featherweight_runtime::{async_host_store, AssemblyDef, CoreWasmEngine, Runtime};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use structfs_core_store::{
    path, DetachedFuture, DetachedReader, DetachedWriter, Path, Record, Value,
};
use tokio::sync::Semaphore;

struct Gate {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
impl DetachedReader for Gate {
    fn read_detached(&mut self, _: &Path) -> DetachedFuture<Option<Record>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            entered.add_permits(1);
            release.acquire().await.unwrap().forget();
            Ok(None)
        })
    }
}
impl DetachedWriter for Gate {
    fn write_detached(&mut self, to: &Path, _: Record) -> DetachedFuture<Path> {
        let to = to.clone();
        Box::pin(async move { Ok(to) })
    }
}

// One request per fresh session, so the response identity is always zero.
// Manifest inspection dirties memory; run verifies that its memory is fresh.
const GUEST: &str = r#"(module
 (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
 (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
 (memory (export "memory") 1)
 (func (export "block_alloc") (param i32) (result i32) (i32.const 4096))
 (data (i32.const 32) "{\"serialization\":\"application/json\"}")
 (data (i32.const 128) "iso/server/requests")
 (data (i32.const 192) "gate/wait")
 (data (i32.const 256) "iso/server/responses/0")
 (data (i32.const 320) "{\"result\":\"ok\",\"value\":42}")
 (func (export "manifest") (param $ret i32) (result i32)
  (i32.store (i32.const 512) (i32.const 99))
  (i32.store (local.get $ret) (i32.const 32))
  (i32.store offset=4 (local.get $ret) (i32.const 36)) (i32.const 0))
 (func (export "run") (result i32)
  (if (i32.ne (i32.load (i32.const 512)) (i32.const 0)) (then unreachable))
  (i32.store (i32.const 512) (i32.const 1))
  (drop (call $read (i32.const 128) (i32.const 19) (i32.const 1024)))
  (drop (call $read (i32.const 192) (i32.const 9) (i32.const 1024)))
  (call $write (i32.const 256) (i32.const 22) (i32.const 320) (i32.const 26) (i32.const 1024))))"#;

fn sample_process(label: &str) {
    #[cfg(unix)]
    {
        if std::env::var_os("FW_CAPACITY").is_none() {
            return;
        }
        let pid = std::process::id().to_string();
        let rss = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output();
        #[cfg(target_os = "macos")]
        let thread_flag = "-M";
        #[cfg(not(target_os = "macos"))]
        let thread_flag = "-L";
        let threads = std::process::Command::new("ps")
            .args([thread_flag, "-p", &pid])
            .output();
        let (Ok(rss), Ok(threads)) = (rss, threads) else {
            eprintln!("sample={label} process metrics unavailable");
            return;
        };
        if rss.status.success() && threads.status.success() {
            eprintln!(
                "sample={label} rss_kib={} threads={}",
                String::from_utf8_lossy(&rss.stdout).trim(),
                String::from_utf8_lossy(&threads.stdout)
                    .lines()
                    .count()
                    .saturating_sub(1)
            );
        }
    }
    let _ = label;
}

#[test]
fn fresh_sessions_share_code_and_release_registrations() {
    let count: usize = std::env::var("FW_CAPACITY")
        .ok()
        .map(|n| n.parse().unwrap())
        .unwrap_or(128);
    assert!(count > 0);
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let engine = CoreWasmEngine::new(2).unwrap();
        let code = Arc::new(engine.prepare(GUEST.as_bytes().to_vec()).await.unwrap());
        let mut runtime = Runtime::new().with_timeout(Duration::from_secs(120));
        runtime.register_core_artifact("prepared:capacity", code);
        let def = AssemblyDef::from_str(
            r#"{
          "assembly":"capacity", "blocks":{"server":"prepared:capacity"},
          "public":"server", "imports":{"gate":"Test synchronization"},
          "wiring":["server:/gate -> $gate"]
        }"#,
        )
        .unwrap();
        sample_process("prepared");
        for round in 0..2 {
            let started = Instant::now();
            let entered = Arc::new(Semaphore::new(0));
            let release = Arc::new(Semaphore::new(0));
            let mut sessions = Vec::with_capacity(count);
            let mut requests = tokio::task::JoinSet::new();
            for _ in 0..count {
                let imports = HashMap::from([(
                    "gate".into(),
                    async_host_store(Gate {
                        entered: entered.clone(),
                        release: release.clone(),
                    }),
                )]);
                let session = runtime
                    .instantiate(&def, imports, "/no-artifact-files-needed".as_ref())
                    .unwrap();
                let request = session.clone();
                requests.spawn(async move { request.read(path!("answer")).await });
                sessions.push(session);
            }
            tokio::time::timeout(
                Duration::from_secs(90),
                entered.acquire_many(count.try_into().unwrap()),
            )
            .await
            .expect("not all requests reached the provider")
            .unwrap()
            .forget();
            assert_eq!(runtime.registered_blocks(), count);
            eprintln!(
                "round={round} parked={count} elapsed_ms={} pid={}",
                started.elapsed().as_millis(),
                std::process::id()
            );
            sample_process("parked");
            release.add_permits(count);
            while let Some(answer) = requests.join_next().await {
                assert_eq!(answer.unwrap().unwrap(), Some(Value::from(42i64)));
            }
            for session in sessions {
                session.shutdown(Duration::from_secs(1)).await;
            }
            assert_eq!(runtime.registered_blocks(), 0);
            sample_process("released");
            // Each provider future and store released its shared state.
            assert_eq!(Arc::strong_count(&entered), 1);
            assert_eq!(Arc::strong_count(&release), 1);
            eprintln!(
                "round={round} completed={count} retained_blocks=0 elapsed_ms={}",
                started.elapsed().as_millis()
            );
        }
    });
}

#[tokio::test]
async fn cancellation_releases_parked_guests_and_admission_waiters() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(engine.prepare(GUEST.as_bytes().to_vec()).await.unwrap());
    let mut runtime = Runtime::new();
    runtime.register_core_artifact("prepared:capacity", code);
    let def = AssemblyDef::from_str(
        r#"{
        "assembly":"cancel", "blocks":{"server":"prepared:capacity"},
        "public":"server", "imports":{"gate":"Test synchronization"},
        "wiring":["server:/gate -> $gate"]
    }"#,
    )
    .unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let mut sessions = Vec::new();
    let mut requests = tokio::task::JoinSet::new();
    for index in 0..2 {
        let imports = HashMap::from([(
            "gate".into(),
            async_host_store(Gate {
                entered: entered.clone(),
                release: release.clone(),
            }),
        )]);
        let session = runtime
            .instantiate(&def, imports, "/missing".as_ref())
            .unwrap();
        let request = session.clone();
        requests.spawn(async move { request.read(path!("answer")).await });
        sessions.push(session);
        if index == 0 {
            tokio::time::timeout(Duration::from_secs(5), entered.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
        }
    }
    // The first guest owns the only store slot; the second cannot enter its provider.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), entered.acquire())
            .await
            .is_err()
    );
    // Cancel the admission waiter first, then the indefinitely parked provider.
    for session in sessions.into_iter().rev() {
        tokio::time::timeout(Duration::from_secs(2), session.shutdown(Duration::ZERO))
            .await
            .unwrap();
    }
    while let Some(result) = requests.join_next().await {
        assert!(result.unwrap().is_err());
    }
    assert_eq!(runtime.registered_blocks(), 0);
    assert_eq!(Arc::strong_count(&entered), 1);
    assert_eq!(Arc::strong_count(&release), 1);
    // Cancellation returned the permit, including the cancelled manifest-free run.
    tokio::time::timeout(
        Duration::from_secs(2),
        engine.prepare(GUEST.as_bytes().to_vec()),
    )
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn whole_assembly_reservation_keeps_capacity_for_lazy_dependency() {
    let engine = CoreWasmEngine::with_limits(1, 2, 65536).unwrap();
    let gateway = engine.prepare(GUEST.as_bytes().to_vec()).await.unwrap();
    let worker = engine
        .prepare(
            GUEST
                .replace("gate/wait", "iso/self/name")
                .replace("(i32.const 9)", "(i32.const 13)")
                .into_bytes(),
        )
        .await
        .unwrap();
    let reservation = engine.reserve_session(2).unwrap();
    assert!(engine.reserve_session(1).is_err());
    let mut runtime = Runtime::new();
    runtime.register_core_artifact("gateway", gateway.in_session(reservation.clone()).unwrap());
    runtime.register_core_artifact("worker", worker.in_session(reservation.clone()).unwrap());
    drop(reservation);
    let def = AssemblyDef::from_str(
        r#"{"assembly":"reserved",
        "blocks":{"server":"gateway","worker":"worker"},"public":"server",
        "wiring":["server:/gate -> worker"]}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), ".".as_ref())
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), assembly.read(path!("answer")))
            .await
            .unwrap()
            .unwrap(),
        Some(Value::from(42i64))
    );
    assembly.shutdown(Duration::ZERO).await;
    assert_eq!(runtime.registered_blocks(), 0);
    drop(assembly);
    drop(runtime);
    assert!(engine.reserve_session(2).is_ok());
}
