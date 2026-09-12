//! Compiled as an independent consumer, including by the package release gate.
#[cfg(test)]
mod tests {
    #[test]
    fn released_value_profiles_preserve_application_state() {
        use structfs_core_store::Codec;
        use structfs_serde_store::{
            from_value, to_value, transcode, ExplicitOption, Profile, ValueCodec,
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
        let source = ValueCodec::new(Profile::ValueJson).canonical();
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
        let budget = CallBudget::<String>::new(CallLimits::default());
        let router = Router::new(vec![]).unwrap();
        let _registration = router
            .register(
                &owner.handle(),
                Mount::new(
                    path!("service"),
                    path!("tenant"),
                    Arc::new(ImmediateStore::new(structfs_core_store::MemoryStore::new())),
                    Arc::new(BudgetAdmission {
                        budget: budget.clone(),
                        key: "state".into(),
                    }),
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
        assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
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
        let global = CallBudget::new(CallLimits::default());
        let mut runtime = Runtime::new().with_call_budget(global.clone());
        runtime.register_artifact("external:fixture", Arc::new(ExternalDriver));
        let (guest, peer) = DuplexStream::pair(4).unwrap();
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
        let denied = instance.request(
            Duration::from_secs(2),
            CallLimits {
                calls: 0,
                ..CallLimits::default()
            },
        );
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
        let meter = instance.public_cell().usage.clone();
        instance.shutdown(Duration::from_secs(1)).await;
        assert_eq!(runtime.registered_blocks(), 0);
        assert!(meter.snapshot().finished);
        assert_eq!(meter.snapshot().counters["operations"].value, 3);
        assert_eq!(global.usage().calls, 0);
        drop(instance);
        drop(runtime);
        drop(guest);
        assert!(peer.readiness().released);
    }
}
