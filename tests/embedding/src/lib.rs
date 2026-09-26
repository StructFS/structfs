//! Compiled as an independent consumer, including by the package release gate.
pub mod lifecycle;

#[cfg(test)]
mod tests {
    #[test]
    fn released_value_profiles_preserve_application_state() {
        use structfs_core_store::Codec;
        use structfs_serde_store::{
            from_value, to_value, transcode, CodecProfile as Profile, ExplicitOption, ValueCodec,
        };
        let state = Value::Map(std::collections::BTreeMap::from([
            ("revision".into(), to_value(&u64::MAX).unwrap()),
            (
                "selection".into(),
                to_value(&ExplicitOption(Some(()))).unwrap(),
            ),
            ("payload".into(), Value::Bytes(vec![0, 255])),
            ("present".into(), Value::Null),
        ]));
        let source = ValueCodec::new(Profile::ValueJson).canonical().unwrap();
        let bytes = source.encode(&state, &source.profile.format()).unwrap();
        for profile in [Profile::ValueJson, Profile::Cbor, Profile::Flexbuffers] {
            let target = ValueCodec::new(profile);
            let encoded = transcode(&bytes, &source, &target).unwrap();
            let decoded = target.decode(&encoded, &profile.format()).unwrap();
            assert!(state.semantic_eq(&decoded));
            assert_eq!(
                from_value::<u64>(decoded.get(&path!("revision")).unwrap().clone()).unwrap(),
                u64::MAX
            );
        }
    }

    use featherweight_runtime::*;
    use std::{collections::HashMap, sync::Arc, time::Duration};
    use structfs_core_store::{path, AsyncReader, AsyncWriter, NoCodec, Record, Value};
    use structfs_handles::{CancelToken, DuplexStream};

    #[tokio::test]
    async fn released_native_router_is_a_store_client() {
        use structfs_service::{
            BudgetAdmission, CallBudget, CallLimits, CleanupSupervisor, ImmediateStore, Mount,
            OwnedTail, OwnerLimits, Permissions, Router,
        };
        let supervisor = CleanupSupervisor::new(1).unwrap();
        let owner = supervisor.owner(OwnerLimits::default()).unwrap();
        let budget = CallBudget::<String>::shared(CallLimits::default());
        let router = Router::shared(vec![]).unwrap();
        let _registration = router
            .register(
                &owner.handle(),
                Mount::new(
                    path!("service"),
                    path!("tenant"),
                    Arc::new(ImmediateStore::new(structfs_core_store::MemoryStore::new())),
                    Arc::new(BudgetAdmission::new(budget.clone(), "state")),
                ),
            )
            .unwrap();
        let client = router
            .client()
            .scoped(&path!("service"), Permissions::READ_WRITE);
        assert_eq!(
            client
                .write(
                    &path!("revision"),
                    Record::parsed(Value::Unsigned(u64::MAX))
                )
                .await
                .unwrap(),
            path!("revision")
        );
        assert_eq!(
            client
                .read(&path!("revision"))
                .await
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::Unsigned(u64::MAX))
        );
        assert_eq!(budget.metrics().admitted, 2);
        assert_eq!(budget.usage().calls, 0);
        let tail = OwnedTail::new(&owner.handle(), 1, 8).unwrap();
        tail.push(vec![0, 255]).unwrap();
        assert!(tail.push(vec![1]).is_err());
        tail.finish();
        assert!(tail.read(0, 1, &CancelToken::new()).await.unwrap().done);
        assert!(owner.join(Duration::from_secs(1)).await.is_quiescent());
        assert!(client.read(&path!("revision")).await.is_err());
        assert!(tail.read(0, 1, &CancelToken::new()).await.is_err());
    }

    struct ExternalDriver;
    impl WasmBlockDriver for ExternalDriver {
        fn manifest(&self) -> Result<Vec<u8>> {
            Ok(br#"{"serialization":"application/json"}"#.to_vec())
        }
        fn execute(
            self: Arc<Self>,
            mut cx: DriverContext,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<i32>> + Send>> {
            Box::pin(async move {
                let work = async {
                    let mut count = 0;
                    loop {
                        let next = cx
                            .namespace
                            .read_async(&path!("iso/server/requests"))
                            .await?;
                        let Some(next) = next else {
                            return Ok::<_, structfs_core_store::Error>(0);
                        };
                        let value = next.into_value(&NoCodec)?;
                        if value.is_null() {
                            return Ok(0);
                        }
                        let request = protocol::RequestEnvelope::from_value(&value)?;
                        if request.path == path!("capabilities") {
                            let caps = cx
                                .namespace
                                .read_async(&path!("iso/capabilities"))
                                .await?
                                .unwrap();
                            cx.namespace
                                .write_async(
                                    &request.respond_to,
                                    Record::parsed(protocol::ok_value(caps.into_value(&NoCodec)?)),
                                )
                                .await?;
                            continue;
                        }
                        count += 1;
                        cx.usage.counter("operations", "requests", count)?;
                        // A request abandoned by its caller does not stop this loop.
                        if request.path == path!("abandon") {
                            let cancel = cx.namespace.request_cancellation(&request.respond_to)?;
                            let receive = path!("stream/rx/4");
                            tokio::select! { biased;
                                _ = cancel.cancelled() => {},
                                _ = cx.namespace.read_async(&receive) => panic!("request consumed unexpected input"),
                            }
                            continue;
                        }
                        let answer = if request.path == path!("binary") {
                            cx.namespace
                                .read_async(&path!("stream/rx/4"))
                                .await?
                                .unwrap()
                                .into_value(&NoCodec)?
                        } else {
                            Value::Integer(count as i64)
                        };
                        cx.namespace
                            .write_async(
                                &request.respond_to,
                                Record::parsed(protocol::ok_value(answer)),
                            )
                            .await?;
                    }
                };
                tokio::select! {
                    result = work => result.map_err(|e| RuntimeError::wasm("external", e)),
                    _ = cx.cancel.cancelled() => Ok(0),
                }
            })
        }
    }

    #[tokio::test]
    async fn external_prepared_driver_persistent_requests_binary_io_and_teardown() {
        let global = CallBudget::shared(CallLimits::default());
        let mut config =
            RuntimeConfig::new(tokio::runtime::Handle::current()).with_call_budget(global.clone());
        config.register_artifact("external:fixture", Arc::new(ExternalDriver));
        let runtime = Runtime::new(config);
        let (guest, peer) = DuplexStream::pair(4).unwrap();
        let guest = Arc::new(guest);
        let def = AssemblyDef::from_str(
            r#"{
            "assembly":"external","imports":{"stream":"bounded binary stream"},
            "blocks":{"server":"external:fixture"}, "public":"server",
            "wiring":["server:/stream -> $stream"]}
        "#,
        )
        .unwrap();
        let instance = runtime
            .instantiate(
                &def,
                HashMap::from([("stream".into(), async_host_store(guest.store()))]),
                ".".as_ref(),
            )
            .unwrap();
        let request = instance.request(Duration::from_millis(20), CallLimits::default());
        assert!(matches!(
            request.read(path!("abandon")).await,
            Err(structfs_core_store::Error::DeadlineExceeded { .. })
        ));
        assert_eq!(request.budget().usage().calls, 0);
        // The server's pending native stream read is charged independently.
        // Allow it to observe request cancellation and drop that read before
        // checking global quiescence; returning to the caller is not a join.
        tokio::time::timeout(Duration::from_secs(1), async {
            while global.usage().calls != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("canceled provider read must release its admission");
        drop(request);
        let request = instance.request(Duration::from_secs(2), CallLimits::default());
        let bytes = vec![0, 255, 128, 10];
        peer.write(&bytes, &CancelToken::new()).await.unwrap();
        assert_eq!(
            request.read(path!("binary")).await.unwrap(),
            Some(Value::Bytes(bytes))
        );
        let denied = instance.request(Duration::from_secs(2), CallLimits::default().with_calls(0));
        assert!(matches!(
            denied.read(path!("denied")).await,
            Err(structfs_core_store::Error::Overloaded { .. })
        ));
        assert!(request.read(path!("again")).await.is_ok());
        assert_eq!(
            request.read(path!("capabilities")).await.unwrap(),
            Some(Value::Array(vec![Value::String("stream".into())]))
        );
        drop(request);
        let server = instance.public_cell();
        instance.shutdown(Duration::from_secs(1)).await;
        assert_eq!(runtime.registered_blocks(), 0);
        assert!(server.usage().finished);
        assert_eq!(server.usage().counters["operations"].value, 3);
        assert_eq!(global.usage().calls, 0);
        drop(instance);
        drop(runtime);
        drop(guest);
        assert!(peer.readiness().closed);
    }
}

/// Owned execution as an embedder sees it: prepare core-Wasm once, start it
/// with `start_async` under an explicit `CleanupSupervisor`, and get the host
/// state back from `ExecutionOwner::join` — after success and after `close`.
#[cfg(test)]
mod owned_execution_consumer {
    use featherweight_runtime::{CoreWasmBlock, CoreWasmEngine, ExecutionPolicy, RuntimeError};
    use std::{sync::Arc, time::Duration};
    use structfs_core_store::{path, Format, MemoryStore, NoCodec, Reader, SyncToAsync, Value};
    use structfs_serde_store::JsonCodec;
    use structfs_service::CleanupSupervisor;

    /// A guest that writes JSON `1` to `effect`, then runs `body`.
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
      (func (export "run") (result i32)
        i32.const 100 i32.const 6 i32.const 200 i32.const 1 i32.const 1024 call $write drop
        {body}))"#
        )
        .into_bytes()
    }

    async fn prepare(engine: &Arc<CoreWasmEngine>, body: &str) -> Arc<CoreWasmBlock> {
        Arc::new(engine.prepare(guest(body)).await.unwrap())
    }

    fn effects(host: &SyncToAsync<MemoryStore>) -> Option<Value> {
        host.inner()
            .lock()
            .unwrap()
            .read(&path!("effect"))
            .unwrap()
            .map(|r| r.into_value(&NoCodec).unwrap())
    }

    #[tokio::test]
    async fn start_async_returns_host_state_on_join_after_success_and_close() {
        let engine = CoreWasmEngine::new(1).unwrap();
        let finishes = prepare(&engine, "i32.const 0").await;
        let spins = prepare(&engine, "(loop $again br $again) i32.const 0").await;
        let supervisor = CleanupSupervisor::new(2).unwrap();

        // A successful run hands the host back with the guest's effect in it.
        let mut run = finishes
            .start_async(
                &supervisor,
                SyncToAsync::new(MemoryStore::new()),
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        let outcome = run.join().await.unwrap();
        assert_eq!(outcome.result.unwrap(), 0);
        assert!(!outcome.host_panicked);
        assert!(outcome.usage.finished);
        assert!(matches!(
            effects(&outcome.host),
            Some(Value::Integer(1) | Value::Unsigned(1))
        ));
        // Host state is returned exactly once.
        assert!(matches!(run.join().await, Err(RuntimeError::AlreadyJoined)));

        // Reuse the recovered host for a run that never finishes by itself:
        // `close` requests cleanup, `join` waits and still returns the host.
        let mut run = spins
            .start_async(
                &supervisor,
                outcome.host,
                JsonCodec,
                Format::JSON,
                ExecutionPolicy::default(),
            )
            .unwrap();
        assert!(run.wait(Duration::from_millis(20)).await.unwrap().is_none());
        run.close();
        let outcome = run.join().await.unwrap();
        assert!(outcome.result.is_err());
        assert!(!outcome.host_panicked);
        assert!(effects(&outcome.host).is_some());
        assert!(supervisor
            .join(Duration::from_secs(1))
            .await
            .iter()
            .all(|report| report.is_quiescent()));
    }
}

#[cfg(test)]
mod state_consumer {
    use std::{collections::BTreeMap, sync::Arc, time::Duration};
    use structfs_core_store::{path, Reader, Value};
    use structfs_service::{
        BudgetAdmission, CallBudget, CallLimits, CleanupSupervisor, Mount, Router,
    };
    use structfs_state::{
        ClientError, Command, Fault, Mutation, ReadLimits, State, StateClient, StateLimits,
    };
    #[tokio::test]
    async fn packaged_state_supports_projection_and_conditional_effect_results() {
        let supervisor = CleanupSupervisor::new(2).unwrap();
        let service_owner = supervisor.owner(Default::default()).unwrap();
        let request = supervisor.owner(Default::default()).unwrap();
        let state = State::shared(
            &service_owner.handle(),
            Some(Value::Map(BTreeMap::new())),
            StateLimits::default(),
        )
        .unwrap();
        let raw = Router::shared(vec![Mount::new(
            path!(""),
            path!(""),
            state.view(path!(""), true),
            Arc::new(BudgetAdmission::new(
                CallBudget::<String>::shared(CallLimits::default()),
                "state",
            )),
        )])
        .unwrap()
        .client()
        .owned_by(&request.handle());
        let client = StateClient::new(raw);
        let generation = client
            .batch(
                None,
                vec![Mutation::Set {
                    path: "generation".into(),
                    value: Value::Unsigned(u64::MAX),
                }],
            )
            .await
            .unwrap();
        let observation = client
            .open(Command::Observe {
                prefix: "".into(),
                limits: ReadLimits::default(),
            })
            .await
            .unwrap();
        let mut view = observation.projection(65536, 128).await.unwrap();
        assert_eq!(
            view.read(&path!("generation")).unwrap().unwrap().as_value(),
            Some(&Value::Unsigned(u64::MAX))
        );
        client
            .batch(
                Some(generation.clone()),
                vec![Mutation::Set {
                    path: "generation".into(),
                    value: Value::Integer(2),
                }],
            )
            .await
            .unwrap();
        // A superseded effect must revalidate; cancellation alone is not a fence.
        assert!(matches!(
            client
                .batch(
                    Some(generation.clone()),
                    vec![Mutation::Set {
                        path: "result".into(),
                        value: Value::String("stale".into())
                    }]
                )
                .await,
            Err(ClientError::State(Fault::Conflict { .. }))
        ));
        assert_eq!(client.data(&path!("result")).await.unwrap(), None);
        assert_eq!(
            observation.changes(&generation).await.unwrap().items.len(),
            1
        );
        assert!(request.join(Duration::from_secs(1)).await.is_quiescent());
        assert!(service_owner
            .join(Duration::from_secs(1))
            .await
            .is_quiescent());
    }
}
