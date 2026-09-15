//! Reproducible native scheduling sample, not a downstream latency claim.
use featherweight_runtime::{CoreWasmEngine, ExecutionPolicy, NoOpStore};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use structfs_core_store::{Format, NoCodec};
use structfs_service::CleanupSupervisor;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let started = Instant::now();
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(
        engine
            .prepare(
                br#"(module
        (memory (export "memory") 1) (data (i32.const 0) "{}")
        (func (export "block_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "manifest") (param $p i32) (result i32)
          local.get $p i32.const 0 i32.store
          local.get $p i32.const 4 i32.add i32.const 2 i32.store i32.const 0)
        (func (export "run") (result i32) i32.const 0))"#
                    .to_vec(),
            )
            .await
            .unwrap(),
    );
    println!("prepare_us={}", started.elapsed().as_micros());
    let supervisor = CleanupSupervisor::new(1).unwrap();
    for synchronous in [true, false] {
        let started = Instant::now();
        for _ in 0..100 {
            let mut run = if synchronous {
                code.start_sync(
                    &supervisor,
                    NoOpStore,
                    NoCodec,
                    Format::OCTET_STREAM,
                    ExecutionPolicy::default(),
                )
            } else {
                code.start_async(
                    &supervisor,
                    NoOpStore,
                    NoCodec,
                    Format::OCTET_STREAM,
                    ExecutionPolicy::default(),
                )
            }
            .unwrap();
            assert_eq!(run.join().await.unwrap().result.unwrap(), 0);
        }
        println!(
            "synchronous={synchronous} runs=100 elapsed_us={}",
            started.elapsed().as_micros()
        );
    }
    assert!(supervisor
        .close(Duration::from_secs(1))
        .await
        .iter()
        .all(|r| r.is_quiescent()));
}
