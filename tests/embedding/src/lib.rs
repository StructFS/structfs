//! Compiled as an independent consumer, including by the package release gate.
#[cfg(test)]
mod tests {
    use featherweight_runtime::*;
    use std::{collections::HashMap, sync::Arc, time::Duration};
    use structfs_core_store::{path, AsyncReader, AsyncWriter, NoCodec, Record, Value};
    use structfs_handles::{CancelToken, DuplexStream};

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
        assert_eq!(global.usage().calls, 0);
        assert_eq!(request.budget().usage().calls, 0);
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
