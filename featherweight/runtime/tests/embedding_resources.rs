use featherweight_runtime::*;
use std::{collections::HashMap, sync::Arc, time::Duration};

#[tokio::test]
async fn prepared_driver_reports_fuel_and_memory_on_success_and_trap() {
    for trap in [false, true] {
        let engine = CoreWasmEngine::with_limits(1, 1, 3 * 65536).unwrap();
        let tail = if trap { "(loop $spin (br $spin))" } else { "" };
        let wat = format!(
            r#"(module
            (memory (export "memory") 1 3)
            (data (i32.const 16) "{{\"serialization\":\"application/json\"}}")
            (func (export "block_alloc") (param i32) (result i32) (i32.const 4096))
            (func (export "manifest") (param i32) (result i32)
                (i32.store (local.get 0) (i32.const 16))
                (i32.store offset=4 (local.get 0) (i32.const 36)) (i32.const 0))
            (func (export "run") (result i32)
                (drop (memory.grow (i32.const 1))) {tail} (i32.const 0)))"#
        );
        let code = Arc::new(engine.prepare(wat.into_bytes()).await.unwrap());
        let mut runtime = Runtime::new().with_metering(Metering {
            fuel: Some(1000000),
            ..Metering::default()
        });
        runtime.register_core_artifact("metered", code);
        let def = AssemblyDef::from_str(
            r#"{"assembly":"metered","blocks":{"a":"metered"},"public":"a"}"#,
        )
        .unwrap();
        let assembly = runtime
            .instantiate(&def, HashMap::new(), ".".as_ref())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), assembly.wait_public_terminal())
            .await
            .unwrap();
        let usage = assembly.public_cell().usage.snapshot();
        assert_eq!(
            usage.linear_memory_bytes,
            Some(0),
            "{:?}",
            assembly.public_cell().last_error()
        );
        assert_eq!(usage.peak_linear_memory_bytes, Some(2 * 65536));
        assert_eq!(usage.linear_memory_limit_bytes, Some(3 * 65536));
        assert_eq!(usage.wasm_fuel_limit, Some(1000000));
        assert!(usage.wasm_fuel_consumed.unwrap() > 0);
        assert!(usage.wasm_fuel_consumed.unwrap() <= 1000000);
        assert!(usage.finished);
        assert_eq!(
            assembly.public_cell().state(),
            if trap {
                BlockState::Failed
            } else {
                BlockState::Stopped
            }
        );
        assembly.shutdown(Duration::ZERO).await;
        assert_eq!(runtime.registered_blocks(), 0);
    }
}

struct PanicDriver;
impl WasmBlockDriver for PanicDriver {
    fn manifest(&self) -> Result<Vec<u8>> {
        Ok(br#"{"serialization":"application/json"}"#.to_vec())
    }
    fn execute(
        self: Arc<Self>,
        _: DriverContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i32>> + Send>> {
        Box::pin(async { panic!("adapter panic fixture") })
    }
}
#[tokio::test]
async fn adapter_panic_is_terminal_and_reclaimable() {
    let mut runtime = Runtime::new();
    runtime.register_artifact("panic", Arc::new(PanicDriver));
    let def = AssemblyDef::from_str(r#"{"assembly":"panic","blocks":{"a":"panic"},"public":"a"}"#)
        .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), ".".as_ref())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), assembly.wait_public_terminal())
        .await
        .unwrap();
    assert_eq!(assembly.public_cell().state(), BlockState::Failed);
    assembly.shutdown(Duration::ZERO).await;
    assert_eq!(runtime.registered_blocks(), 0);
}
