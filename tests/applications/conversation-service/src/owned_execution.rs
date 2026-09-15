use featherweight_runtime::{CoreWasmBlock, CoreWasmEngine, ExecutionPolicy, GrowthFailure};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};
use structfs_core_store::{Error, Format, Path, Reader, Record, Writer};
use structfs_serde_store::JsonCodec;
use structfs_service::CleanupSupervisor;

fn guest(body: &str) -> Vec<u8> {
    format!(
        r#"(module
      (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "{{}}") (data (i32.const 100) "effect") (data (i32.const 200) "1")
      (func (export "block_alloc") (param i32) (result i32) i32.const 2048)
      (func (export "manifest") (param $ret i32) (result i32)
        local.get $ret i32.const 0 i32.store
        local.get $ret i32.const 4 i32.add i32.const 2 i32.store i32.const 0)
      (func (export "run") (result i32) {body}))"#
    )
    .into_bytes()
}
const WRITE: &str =
    "i32.const 100 i32.const 6 i32.const 200 i32.const 1 i32.const 1024 call $write drop";
#[derive(Default)]
struct Host {
    effects: Vec<Path>,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<Mutex<mpsc::Receiver<()>>>,
    panic: bool,
}
impl Reader for Host {
    fn read(&mut self, _: &Path) -> Result<Option<Record>, Error> {
        Ok(None)
    }
}
impl Writer for Host {
    fn write(&mut self, path: &Path, _: Record) -> Result<Path, Error> {
        if let Some(entered) = &self.entered {
            entered.notify_one();
        }
        if let Some(release) = self.release.take() {
            release.into_inner().unwrap().recv().unwrap();
        }
        self.effects.push(path.clone());
        assert!(!self.panic, "host failure after effect");
        Ok(path.clone())
    }
}
async fn prepare(engine: &Arc<CoreWasmEngine>, body: &str) -> Arc<CoreWasmBlock> {
    Arc::new(engine.prepare(guest(body)).await.unwrap())
}

#[tokio::test]
async fn prepared_sync_reuses_code_recovers_nonclone_state_and_fresh_memory() {
    let engine =
        CoreWasmEngine::with_epoch_interval(1, 2, 65536, Duration::from_millis(5)).unwrap();
    let code = prepare(
        &engine,
        &format!("{WRITE} i32.const 4096 i32.load i32.const 4096 i32.const 99 i32.store"),
    )
    .await;
    let supervisor = CleanupSupervisor::new(4).unwrap();
    let mut host = Host::default();
    for count in 1..=3 {
        let mut run = code
            .start_sync(
                &supervisor,
                host,
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        let outcome = run.join().await.unwrap();
        assert_eq!(outcome.result.unwrap(), 0);
        assert!(!outcome.host_panicked);
        assert!(outcome.usage.finished);
        assert_eq!(outcome.usage.linear_memory_bytes, Some(0));
        host = outcome.host;
        assert_eq!(host.effects.len(), count);
    }
    assert!(supervisor
        .close(Duration::from_secs(1))
        .await
        .iter()
        .all(|r| r.is_quiescent()));
}

#[tokio::test]
async fn trap_and_host_panic_recover_accepted_effects() {
    let engine = CoreWasmEngine::new(1).unwrap();
    let code = prepare(&engine, &format!("{WRITE} unreachable")).await;
    let supervisor = CleanupSupervisor::new(4).unwrap();
    for panic in [false, true] {
        let mut run = code
            .start_sync(
                &supervisor,
                Host {
                    panic,
                    ..Host::default()
                },
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        let outcome = run.join().await.unwrap();
        assert!(outcome.result.is_err());
        assert_eq!(outcome.host.effects.len(), 1);
        assert_eq!(outcome.host_panicked, panic);
    }
}

#[tokio::test]
async fn cancellation_keeps_blocking_effect_owned_until_joined() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = prepare(&engine, &format!("{WRITE} i32.const 0")).await;
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let (send, recv) = mpsc::channel();
    let host = Host {
        entered: Some(entered.clone()),
        release: Some(Mutex::new(recv)),
        ..Host::default()
    };
    let mut run = code
        .start_sync(
            &supervisor,
            host,
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    entered.notified().await;
    assert!(run.wait(Duration::ZERO).await.unwrap().is_none());
    run.cancel();
    assert!(run.wait(Duration::from_millis(1)).await.unwrap().is_none());
    assert!(!run.report().is_quiescent());
    let rejected = code
        .start_sync(
            &supervisor,
            Host::default(),
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .err()
        .unwrap();
    assert!(rejected.host.effects.is_empty());
    send.send(()).unwrap();
    let result = run.join().await.unwrap();
    assert!(result.result.is_err());
    assert_eq!(result.host.effects.len(), 1);
}

#[tokio::test]
async fn dropped_owner_transfers_work_to_supervisor() {
    let engine = CoreWasmEngine::new(1).unwrap();
    let code = prepare(&engine, &format!("{WRITE} i32.const 0")).await;
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let (send, recv) = mpsc::channel();
    let run = code
        .start_sync(
            &supervisor,
            Host {
                entered: Some(entered.clone()),
                release: Some(Mutex::new(recv)),
                ..Host::default()
            },
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    entered.notified().await;
    drop(run);
    assert!(supervisor
        .close(Duration::ZERO)
        .await
        .iter()
        .any(|r| !r.is_quiescent()));
    send.send(()).unwrap();
    assert!(supervisor
        .close(Duration::from_secs(2))
        .await
        .iter()
        .all(|r| r.is_quiescent()));
}

#[tokio::test]
async fn growth_policy_fuel_deadline_and_independent_cancellation() {
    let engine = CoreWasmEngine::with_limits(1, 2, 65536).unwrap();
    let grow = prepare(&engine, "i32.const 1 memory.grow drop i32.const 0").await;
    let spin = prepare(&engine, "(loop $again br $again) i32.const 0").await;
    let supervisor = CleanupSupervisor::new(8).unwrap();
    for growth_failure in [GrowthFailure::Trap, GrowthFailure::ReturnFailure] {
        let mut run = grow
            .start_sync(
                &supervisor,
                Host::default(),
                JsonCodec,
                Format::JSON,
                ExecutionPolicy {
                    growth_failure,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            run.join().await.unwrap().result.is_err(),
            growth_failure == GrowthFailure::Trap
        );
    }
    let mut a = spin
        .start_sync(
            &supervisor,
            Host::default(),
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    let mut b = spin
        .start_sync(
            &supervisor,
            Host::default(),
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    a.cancel();
    assert!(a.join().await.unwrap().result.is_err());
    assert!(b.wait(Duration::from_millis(20)).await.unwrap().is_none());
    b.cancel();
    assert!(b.join().await.unwrap().result.is_err());
    for policy in [
        ExecutionPolicy {
            fuel: Some(10),
            ..Default::default()
        },
        ExecutionPolicy {
            deadline: Some(tokio::time::Instant::now()),
            ..Default::default()
        },
        ExecutionPolicy {
            memory_bytes: Some(131072),
            ..Default::default()
        },
    ] {
        let mut run = spin
            .start_sync(
                &supervisor,
                Host::default(),
                JsonCodec,
                Format::JSON,
                policy,
            )
            .unwrap();
        assert!(run.join().await.unwrap().result.is_err());
    }
}

#[async_trait::async_trait]
impl structfs_core_store::AsyncReader for Host {
    async fn read_async(&mut self, p: &Path) -> Result<Option<Record>, Error> {
        self.read(p)
    }
}
#[async_trait::async_trait]
impl structfs_core_store::AsyncWriter for Host {
    async fn write_async(&mut self, p: &Path, r: Record) -> Result<Path, Error> {
        self.write(p, r)
    }
}
#[tokio::test]
async fn async_host_has_same_recovery_and_growth_contract() {
    let engine = CoreWasmEngine::with_limits(1, 2, 65536).unwrap();
    let supervisor = CleanupSupervisor::new(4).unwrap();
    for body in [
        format!("{WRITE} unreachable"),
        "i32.const 1 memory.grow drop i32.const 0".into(),
    ] {
        let code = prepare(&engine, &body).await;
        let mut run = code
            .start_async(
                &supervisor,
                Host::default(),
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        let result = run.join().await.unwrap();
        assert!(result.result.is_err());
        if body.contains("write") {
            assert_eq!(result.host.effects.len(), 1);
        }
    }
}

#[tokio::test]
async fn async_cancellation_joins_an_accepted_host_future() {
    struct AsyncHost {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        writes: usize,
    }
    #[async_trait::async_trait]
    impl structfs_core_store::AsyncReader for AsyncHost {
        async fn read_async(&mut self, _: &Path) -> Result<Option<Record>, Error> {
            Ok(None)
        }
    }
    #[async_trait::async_trait]
    impl structfs_core_store::AsyncWriter for AsyncHost {
        async fn write_async(&mut self, p: &Path, _: Record) -> Result<Path, Error> {
            self.entered.notify_one();
            self.release.notified().await;
            self.writes += 1;
            Ok(p.clone())
        }
    }
    let engine = CoreWasmEngine::new(1).unwrap();
    let code = prepare(&engine, &format!("{WRITE} i32.const 0")).await;
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut run = code
        .start_async(
            &supervisor,
            AsyncHost {
                entered: entered.clone(),
                release: release.clone(),
                writes: 0,
            },
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    entered.notified().await;
    run.cancel();
    assert!(run.wait(Duration::from_millis(1)).await.unwrap().is_none());
    release.notify_one();
    let outcome = run.join().await.unwrap();
    assert!(outcome.result.is_err());
    assert_eq!(outcome.host.writes, 1);
}

#[tokio::test]
async fn tighter_limit_can_fail_instantiation_without_losing_host() {
    let engine = CoreWasmEngine::new(1).unwrap();
    let code = Arc::new(
        engine
            .prepare(guest("i32.const 0").into_iter().collect())
            .await
            .unwrap(),
    );
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let mut run = code
        .start_sync(
            &supervisor,
            Host::default(),
            JsonCodec,
            Format::JSON,
            ExecutionPolicy {
                memory_bytes: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let outcome = run.join().await.unwrap();
    assert!(outcome.result.is_err());
    assert!(outcome.host.effects.is_empty());
}

// Saturate the engine with an accepted effect. Queued cancellations and deadlines
// must recover untouched hosts without waiting for that unrelated effect.
#[tokio::test]
async fn queued_cancellation_storm_recovers_hosts_and_reuses_capacity() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
        let code = prepare(&engine, &format!("{WRITE} {WRITE} i32.const 0")).await;
        let supervisor = CleanupSupervisor::new(33).unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let (send, recv) = mpsc::channel();
        let mut active = code
            .start_sync(
                &supervisor,
                Host {
                    entered: Some(entered.clone()),
                    release: Some(Mutex::new(recv)),
                    ..Host::default()
                },
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        entered.notified().await;
        let mut queued = Vec::new();
        for _ in 0..32 {
            queued.push(
                code.start_sync(
                    &supervisor,
                    Host::default(),
                    JsonCodec,
                    Format::JSON,
                    ExecutionPolicy::default(),
                )
                .unwrap(),
            );
        }
        // Give every admitted task an opportunity to wait for the engine slot.
        tokio::task::yield_now().await;
        for run in &queued {
            run.cancel();
        }
        for mut run in queued {
            let outcome = run.join().await.unwrap();
            assert!(outcome.result.is_err());
            assert!(outcome.host.effects.is_empty());
            assert!(run.report().is_quiescent());
            assert!(run.join().await.is_err());
        }
        let mut expired = code
            .start_sync(
                &supervisor,
                Host::default(),
                JsonCodec,
                Format::JSON,
                ExecutionPolicy {
                    deadline: Some(tokio::time::Instant::now() + Duration::from_millis(10)),
                    ..Default::default()
                },
            )
            .unwrap();
        let outcome = expired.join().await.unwrap();
        assert!(outcome.result.is_err());
        assert!(outcome.host.effects.is_empty());
        assert!(!active.report().is_quiescent());
        active.cancel();
        send.send(()).unwrap();
        let outcome = active.join().await.unwrap();
        assert!(outcome.result.is_err());
        // The accepted write finishes; the second import must never dispatch.
        assert_eq!(outcome.host.effects.len(), 1);
        let mut next = code
            .start_sync(
                &supervisor,
                outcome.host,
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        let outcome = next.join().await.unwrap();
        assert_eq!(outcome.result.unwrap(), 0);
        assert_eq!(outcome.host.effects.len(), 3);
    })
    .await
    .expect("queued runs must not wait for the unrelated blocked host");
}

#[tokio::test]
async fn async_host_panic_recovers_effect_and_releases_admission() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = prepare(&engine, &format!("{WRITE} i32.const 0")).await;
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let mut run = code
        .start_async(
            &supervisor,
            Host {
                panic: true,
                ..Host::default()
            },
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    let outcome = run.join().await.unwrap();
    assert!(outcome.host_panicked);
    assert!(outcome.result.is_err());
    assert_eq!(outcome.host.effects.len(), 1);
    let mut next = code
        .start_async(
            &supervisor,
            Host::default(),
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
        )
        .unwrap();
    assert_eq!(next.join().await.unwrap().result.unwrap(), 0);
}
