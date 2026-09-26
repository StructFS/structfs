use super::*;
use structfs_core_store::{path, MemoryStore, Shared, SyncToAsync, Value};
use structfs_serde_store::JsonCodec;

/// A complete spec 11 guest, hand-written in wat (~40 lines): a bump
/// allocator, a static manifest, and a run() that reads `input`,
/// verifies `missing` is absent, and echoes the data to `output`.
/// This is the "an SDK is an afternoon" claim, demonstrated in the
/// least ergonomic language available.
const ECHO_GUEST: &str = r#"
    (module
      (import "structfs" "read"
        (func $read (param i32 i32 i32) (result i32)))
      (import "structfs" "write"
        (func $write (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 4096))
      (func (export "block_alloc") (param $len i32) (result i32)
        (local $ptr i32)
        (local.set $ptr (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $ptr))
      (data (i32.const 1040) "input")
      (data (i32.const 1056) "missing")
      (data (i32.const 1072) "output")
      (data (i32.const 1088)
        "{\"name\":\"wat-echo\",\"serialization\":\"application/json\"}")
      (func (export "manifest") (param $ret i32) (result i32)
        (i32.store (local.get $ret) (i32.const 1088))
        (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 54))
        (i32.const 0))
      (func (export "run") (result i32)
        (local $st i32)
        ;; read "input" -> ret record at 1024
        (local.set $st
          (call $read (i32.const 1040) (i32.const 5) (i32.const 1024)))
        (if (i32.ne (local.get $st) (i32.const 0))
          (then (return (i32.const 1))))
        ;; read "missing" -> must be status 1 (absent)
        (local.set $st
          (call $read (i32.const 1056) (i32.const 7) (i32.const 1032)))
        (if (i32.ne (local.get $st) (i32.const 1))
          (then (return (i32.const 2))))
        ;; write the input bytes to "output"
        (local.set $st
          (call $write (i32.const 1072) (i32.const 6)
            (i32.load (i32.const 1024)) (i32.load (i32.const 1028))
            (i32.const 1032)))
        (if (i32.ne (local.get $st) (i32.const 0))
          (then (return (i32.const 3))))
        (i32.const 0)))
"#;

/// A guest that reads `secret` and returns the status it saw as its
/// exit code (negated, so it is a positive integer).
const STATUS_GUEST: &str = r#"
    (module
      (import "structfs" "read"
        (func $read (param i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $bump (mut i32) (i32.const 4096))
      (func (export "block_alloc") (param $len i32) (result i32)
        (local $ptr i32)
        (local.set $ptr (global.get $bump))
        (global.set $bump (i32.add (global.get $bump) (local.get $len)))
        (local.get $ptr))
      (data (i32.const 1040) "secret")
      (data (i32.const 1088) "{\"serialization\":\"application/json\"}")
      (func (export "manifest") (param $ret i32) (result i32)
        (i32.store (local.get $ret) (i32.const 1088))
        (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 36))
        (i32.const 0))
      (func (export "run") (result i32)
        (i32.sub (i32.const 0)
          (call $read (i32.const 1040) (i32.const 6) (i32.const 1024)))))
"#;

/// Spins forever: the metering test subject.
const SPIN_GUEST: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "block_alloc") (param i32) (result i32) (i32.const 4096))
      (data (i32.const 1088) "{\"serialization\":\"application/json\"}")
      (func (export "manifest") (param $ret i32) (result i32)
        (i32.store (local.get $ret) (i32.const 1088))
        (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 36))
        (i32.const 0))
      (func (export "run") (result i32)
        (loop $spin (br $spin))
        (i32.const 0)))
"#;

/// Fails every operation with one fixed error.
struct FailStore(fn() -> StoreError);

impl Reader for FailStore {
    fn read(&mut self, _: &Path) -> std::result::Result<Option<Record>, StoreError> {
        Err((self.0)())
    }
}

impl Writer for FailStore {
    fn write(&mut self, _: &Path, _: Record) -> std::result::Result<Path, StoreError> {
        Err((self.0)())
    }
}

async fn prepared(guest: &str) -> Arc<CoreWasmBlock> {
    Arc::new(
        CoreWasmEngine::new(1)
            .unwrap()
            .prepare(guest.as_bytes().to_vec())
            .await
            .unwrap(),
    )
}

fn run<S, C>(
    host: S,
    codec: C,
    format: Format,
    policy: ExecutionPolicy,
    cancel: CancelToken,
) -> HostRun<S, C> {
    HostRun {
        host,
        codec,
        format,
        policy,
        cancel,
        meter: ExecutionMeter::default(),
    }
}

async fn run_sync<S: Reader + Writer + Send + 'static>(
    block: &Arc<CoreWasmBlock>,
    host: S,
    policy: ExecutionPolicy,
    cancel: CancelToken,
) -> ExecutionOutcome<S> {
    block
        .run_host_sync(run(host, JsonCodec, Format::JSON, policy, cancel))
        .await
        .expect("host recovered")
}

#[tokio::test]
async fn manifest_crosses_the_boundary() {
    let block = prepared(ECHO_GUEST).await;
    let json: serde_json::Value = serde_json::from_slice(block.manifest()).unwrap();
    assert_eq!(json["name"], "wat-echo");
    assert_eq!(json["serialization"], "application/json");
}

#[tokio::test]
async fn every_transport_crosses_the_boundary_on_both_linkers() {
    use structfs_serde_store::MultiCodec;

    // The echo guest moves the payload bytes verbatim, so a
    // round trip proves host encode -> guest -> host decode for
    // each transport. The binary transports carry Value::Bytes
    // faithfully — the JSON tier cannot.
    let block = prepared(ECHO_GUEST).await;
    let cases = [
        (Format::JSON, Value::from("hello over json")),
        (Format::CBOR, Value::Bytes(vec![0, 159, 146, 150])),
        (Format::FLEXBUFFERS, Value::Bytes(vec![255, 0, 7])),
        (
            Format::VALUE_JSON,
            Value::Array(vec![
                Value::Unsigned(u64::MAX),
                Value::Bytes(vec![0, 255]),
                Value::Null,
                Value::Float(-0.0),
                Value::Float(f64::NAN),
            ]),
        ),
    ];
    for (format, value) in cases {
        for asynchronous in [false, true] {
            let mut store = Shared::new(MemoryStore::new());
            store
                .write(&path!("input"), Record::parsed(value.clone()))
                .unwrap();
            let host = run(
                store.clone(),
                MultiCodec::standard(),
                format.clone(),
                ExecutionPolicy::default(),
                CancelToken::new(),
            );
            let code = if asynchronous {
                let host = HostRun {
                    host: SyncToAsync::new(host.host),
                    codec: host.codec,
                    format: host.format,
                    policy: host.policy,
                    cancel: host.cancel,
                    meter: host.meter,
                };
                block.run_host_async(host).await.result
            } else {
                block.run_host_sync(host).await.unwrap().result
            };
            assert_eq!(code.unwrap(), 0, "guest failed under {format}");
            let output = store.read(&path!("output")).unwrap().unwrap();
            assert!(
                output.as_value().unwrap().semantic_eq(&value),
                "mangled by {format}"
            );
        }
    }
}

/// Every typed error crosses the boundary as its spec 11 status.
#[tokio::test]
async fn typed_errors_cross_as_status_codes() {
    let block = prepared(STATUS_GUEST).await;
    let cases: [(fn() -> StoreError, i32); 9] = [
        (|| StoreError::not_found(path!("x")), status::NOT_FOUND),
        (
            || StoreError::permission_denied("unwired"),
            status::PERMISSION_DENIED,
        ),
        (|| StoreError::conflict("stale"), status::CONFLICT),
        (|| StoreError::overloaded("busy"), status::OVERLOADED),
        (
            || StoreError::deadline_exceeded("late"),
            status::DEADLINE_EXCEEDED,
        ),
        (|| StoreError::cancelled("stop"), status::CANCELLED),
        (|| StoreError::resource_limit("big"), status::RESOURCE_LIMIT),
        (|| StoreError::store("x", "y", "z"), status::OTHER),
        (
            || StoreError::invalid_argument("bad"),
            status::INVALID_ARGUMENT,
        ),
    ];
    for (error, expected) in cases {
        let outcome = run_sync(
            &block,
            FailStore(error),
            ExecutionPolicy::default(),
            CancelToken::new(),
        )
        .await;
        assert_eq!(outcome.result.unwrap(), -expected, "{}", error());
    }
}

#[tokio::test]
async fn fuel_cap_stops_a_spinning_guest() {
    let block = prepared(SPIN_GUEST).await;
    let outcome = run_sync(
        &block,
        NoOpStore,
        ExecutionPolicy::default().with_fuel(1_000_000),
        CancelToken::new(),
    )
    .await;
    let err = outcome.result.unwrap_err();
    assert!(err.to_string().contains("fuel"), "{err}");
    assert!(outcome.usage.wasm_fuel_consumed.unwrap() >= 1_000_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_interrupts_a_spinning_guest_through_the_engine_ticker() {
    let engine = CoreWasmEngine::with_epoch_interval(
        1,
        4,
        64 * 1024 * 1024,
        std::time::Duration::from_millis(2),
    )
    .unwrap();
    let block = Arc::new(
        engine
            .prepare(SPIN_GUEST.as_bytes().to_vec())
            .await
            .unwrap(),
    );
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run_sync(&block, NoOpStore, ExecutionPolicy::default(), cancel),
    )
    .await
    .expect("the ticker interrupted the guest");
    let err = outcome.result.unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err}");
}

#[test]
fn no_op_store_read_returns_none() {
    let mut store = NoOpStore;
    assert!(store.read(&path!("some/path")).unwrap().is_none());
}

#[test]
fn no_op_store_write_echoes_path() {
    let mut store = NoOpStore;
    let record = Record::raw(bytes::Bytes::from_static(b"data"), Format::OCTET_STREAM);
    assert_eq!(
        store.write(&path!("some/path"), record).unwrap(),
        path!("some/path")
    );
}

#[test]
fn transfer_ranges_are_bounded_before_copying() {
    assert!(checked_range(usize::MAX, 1, usize::MAX).is_err());
    assert!(checked_range(0, MAX_TRANSFER_BYTES + 1, usize::MAX).is_err());
    assert!(checked_range(65535, 2, 65536).is_err());
    assert_eq!(checked_range(65536, 0, 65536).unwrap(), 65536..65536);
}

#[tokio::test]
async fn malformed_import_ranges_trap_on_both_execution_paths() {
    for (ptr, len) in [(0, -1), (-1, 1), (65535, 2)] {
        let guest = format!(
            r#"(module
            (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 512) "{{}}")
            (func (export "block_alloc") (param i32) (result i32) i32.const 1024)
            (func (export "manifest") (param $ret i32) (result i32)
                local.get $ret i32.const 512 i32.store
                local.get $ret i32.const 4 i32.add i32.const 2 i32.store i32.const 0)
            (func (export "run") (result i32)
                (call $read (i32.const {ptr}) (i32.const {len}) (i32.const 0))))"#
        );
        let block = prepared(&guest).await;
        let sync = run_sync(
            &block,
            NoOpStore,
            ExecutionPolicy::default(),
            CancelToken::new(),
        )
        .await
        .result
        .unwrap_err();
        let asynchronous = block
            .run_host_async(run(
                SyncToAsync::new(NoOpStore),
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
                CancelToken::new(),
            ))
            .await
            .result
            .unwrap_err();
        for err in [sync, asynchronous] {
            assert!(
                err.to_string().contains("guest transfer")
                    || err.to_string().contains("out of bounds"),
                "{err}"
            );
        }
    }
}

#[tokio::test]
async fn malformed_manifest_range_is_rejected_before_allocation() {
    let guest = ECHO_GUEST.replace("(i32.const 54)", "(i32.const -1)");
    let err = CoreWasmEngine::new(1)
        .unwrap()
        .prepare(guest.into_bytes())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("guest transfer"), "{err}");
}

#[test]
fn async_echo_runs_with_one_blocking_worker() {
    // A guest occupying the sole blocking worker would deadlock when
    // SyncToAsync dispatches the guest's provider operation to that pool.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut store = Shared::new(MemoryStore::new());
        let value = Value::from("fresh async guest");
        store
            .write(&path!("input"), Record::parsed(value.clone()))
            .unwrap();
        let block = prepared(ECHO_GUEST).await;
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            block.run_host_async(run(
                SyncToAsync::new(store.clone()),
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
                CancelToken::new(),
            )),
        )
        .await
        .expect("guest held the blocking worker");
        assert_eq!(outcome.result.unwrap(), 0);
        assert_eq!(
            store.read(&path!("output")).unwrap().unwrap().as_value(),
            Some(&value)
        );
    });
}

#[tokio::test]
async fn async_spin_yields_to_cancellation_on_current_thread() {
    let block = prepared(SPIN_GUEST).await;
    let cancel = CancelToken::new();
    let trigger = cancel.clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        block.run_host_async(run(
            SyncToAsync::new(NoOpStore),
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
            cancel,
        )),
    )
    .await
    .expect("guest starved the executor");
    timer.await.unwrap();
    let err = outcome.result.unwrap_err();
    assert!(
        err.to_string().contains("interrupted") || err.to_string().contains("cancelled"),
        "{err}"
    );
}

#[tokio::test]
async fn prepared_memory_limits_cover_instantiation_and_growth() {
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let too_large = SPIN_GUEST.replace(
        "(memory (export \"memory\") 1)",
        "(memory (export \"memory\") 2)",
    );
    assert!(engine.prepare(too_large.into_bytes()).await.is_err());
    let guest = SPIN_GUEST.replace(
        "(loop $spin (br $spin))",
        r#"
        (if (i32.ne (memory.grow (i32.const 1)) (i32.const -1))
            (then unreachable))"#,
    );
    let prepared = Arc::new(engine.prepare(guest.into_bytes()).await.unwrap());
    let outcome = prepared
        .run_host_async(run(
            NoOpStore,
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
            CancelToken::new(),
        ))
        .await;
    assert!(outcome.result.is_err());
    let policy = ExecutionPolicy::default().with_memory_bytes(2 * 65536);
    let refused = prepared
        .run_host_async(run(
            NoOpStore,
            JsonCodec,
            Format::JSON,
            policy,
            CancelToken::new(),
        ))
        .await;
    assert!(matches!(refused.result, Err(RuntimeError::Policy(_))));
}

/// A worker that never starts (its executor shut down) is not a panic:
/// the run returns an outcome carrying the untouched host.
#[test]
fn a_run_whose_worker_never_starts_returns_the_host() {
    let engine_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let block = engine_runtime.block_on(prepared(ECHO_GUEST));
    engine_runtime.shutdown_background();
    let caller = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut host = Shared::new(MemoryStore::new());
    host.write(&path!("kept"), Record::parsed(Value::from(true)))
        .unwrap();
    let outcome = caller
        .block_on(block.run_host_sync(run(
            host,
            JsonCodec,
            Format::JSON,
            ExecutionPolicy::default(),
            CancelToken::new(),
        )))
        .expect("the host was never moved into a worker");
    assert!(matches!(
        outcome.result,
        Err(RuntimeError::ExecutionLost(_))
    ));
    assert!(!outcome.host_panicked);
    assert!(outcome.usage.finished);
    let mut host = outcome.host;
    assert!(host.read(&path!("kept")).unwrap().is_some());
}

#[test]
fn component_sniffing() {
    // Core module: version 1, layer 0.
    assert!(!is_component(b"\0asm\x01\x00\x00\x00rest"));
    // Component: version 0x0d, layer 1.
    assert!(is_component(b"\0asm\x0d\x00\x01\x00rest"));
    assert!(!is_component(b"short"));
    assert!(!is_component(b"not wasm"));
}
