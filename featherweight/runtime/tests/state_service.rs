use featherweight_runtime::{service_host_store, AssemblyDef, BlockState, CoreWasmEngine, Runtime};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};
use structfs_core_store::{path, Codec, Value};
use structfs_serde_store::{to_value, Profile, ValueCodec};
use structfs_service::{BudgetAdmission, CallBudget, CallLimits, CleanupSupervisor, Mount, Router};
use structfs_state::{Command, Mutation, ReadLimits, Request, State, StateClient, StateLimits};

#[tokio::test]
async fn guest_observes_then_commits_and_reads_changes_with_owned_handles() {
    let supervisor = CleanupSupervisor::new(1).unwrap();
    let owner = supervisor.owner(Default::default()).unwrap();
    let state = State::new(
        &owner.handle(),
        Some(Value::Map(BTreeMap::new())),
        StateLimits {
            handles: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let codec = ValueCodec::new(Profile::ValueJson);
    let encode = |command| {
        codec
            .encode(
                &to_value(&Request::new(command)).unwrap(),
                &Profile::ValueJson.format(),
            )
            .unwrap()
    };
    let observe = encode(Command::Observe {
        prefix: "".into(),
        limits: ReadLimits::default(),
    });
    let batch = encode(Command::Batch {
        expected: None,
        mutations: vec![Mutation::Set {
            path: "counter".into(),
            value: Value::Unsigned(u64::MAX),
        }],
    });
    let escape = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|b| format!("\\{b:02x}"))
            .collect::<String>()
    };
    let manifest = br#"{"serialization":"application/vnd.structfs.value+json;version=1"}"#;
    let wat = format!(
        r#"(module
      (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
      (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "block_alloc") (param i32) (result i32) (i32.const 32768))
      (data (i32.const 32) "{}")
      (data (i32.const 256) "state/operations")
      (data (i32.const 300) "/snapshot/0")
      (data (i32.const 320) "/changes/0")
      (data (i32.const 512) "{}")
      (data (i32.const 2048) "{}")
      (func (export "manifest") (param $ret i32) (result i32)
        (i32.store (local.get $ret) (i32.const 32))
        (i32.store offset=4 (local.get $ret) (i32.const {})) (i32.const 0))
      (func (export "run") (result i32) (local $length i32)
        (if (call $write (i32.const 256) (i32.const 16) (i32.const 512) (i32.const {}) (i32.const 1024)) (then unreachable))
        (local.set $length (i32.load (i32.const 1028)))
        (memory.copy (i32.const 4096) (i32.load (i32.const 1024)) (local.get $length))
        (memory.copy (i32.add (i32.const 4096) (local.get $length)) (i32.const 300) (i32.const 11))
        (if (call $read (i32.const 4096) (i32.add (local.get $length) (i32.const 11)) (i32.const 1024)) (then unreachable))
        (if (call $write (i32.const 256) (i32.const 16) (i32.const 2048) (i32.const {}) (i32.const 1024)) (then unreachable))
        (memory.copy (i32.add (i32.const 4096) (local.get $length)) (i32.const 320) (i32.const 10))
        (if (call $read (i32.const 4096) (i32.add (local.get $length) (i32.const 10)) (i32.const 1024)) (then unreachable))
        (i32.const 0)))"#,
        escape(manifest),
        escape(&observe),
        escape(&batch),
        manifest.len(),
        observe.len(),
        batch.len()
    );
    let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
    let code = Arc::new(engine.prepare(wat.into_bytes()).await.unwrap());
    let mut runtime = Runtime::new();
    runtime.register_core_artifact("state-guest", code);
    let def=AssemblyDef::from_str(r#"{"assembly":"state","imports":{"state":"revisioned"},"blocks":{"guest":"state-guest"},"public":"guest","wiring":["guest:/state -> $state"]}"#).unwrap();
    let assembly = runtime
        .instantiate(
            &def,
            HashMap::from([(
                "state".into(),
                service_host_store(state.view(path!(""), true)),
            )]),
            ".".as_ref(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), assembly.wait_public_terminal())
        .await
        .unwrap();
    assert_eq!(assembly.public_cell().state(), BlockState::Stopped);
    assert_eq!(state.token().revision, 1);
    let client = StateClient::new(
        Router::new(vec![Mount::new(
            path!(""),
            path!(""),
            state.view(path!(""), true),
            Arc::new(BudgetAdmission {
                budget: CallBudget::<String>::new(CallLimits::default()),
                key: "state".into(),
            }),
        )])
        .unwrap()
        .client(),
    );
    assert_eq!(
        client.data(&path!("counter")).await.unwrap(),
        Some(Value::Unsigned(u64::MAX))
    );
    assert!(client
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: ReadLimits::default()
        })
        .await
        .is_err());
    assert!(assembly.shutdown(Duration::from_secs(1)).await.complete());
    assert!(client
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: ReadLimits::default()
        })
        .await
        .is_ok());
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
}
