//! Independent Horns-shaped fixture: headless input, pure application view,
//! host reducer, native and actual core-Wasm orchestration. No Ox dependency.
#[cfg(test)]
mod tests {
    use featherweight_runtime::{
        service_host_store, AssemblyDef, BlockState, CoreWasmEngine, Runtime,
    };
    use std::{
        collections::{BTreeMap, HashMap},
        sync::Arc,
        time::Duration,
    };
    use structfs_core_store::{path, DetachedFuture, Error, Record, Value};
    use structfs_profiles::{
        Declaration, HeadlessHost, Implementation, Input, InputEnvelope, Profile, Profiled, Session,
    };
    use structfs_serde_store::{from_value, to_value};
    use structfs_service::*;
    use structfs_state::*;
    fn client(service: Arc<dyn Service>, owner: &OwnerHandle, budget: Arc<CallBudget>) -> Client {
        Router::new(vec![Mount::new(
            path!(""),
            path!(""),
            service,
            Arc::new(BudgetAdmission {
                budget,
                key: "screen".into(),
            }),
        )])
        .unwrap()
        .client()
        .owned_by(owner)
    }
    struct Reducer {
        client: StateClient,
        session: Arc<Session>,
    }
    impl Service for Reducer {
        fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
            let client = self.client.clone();
            let session = self.session.clone();
            Box::pin(async move {
                context.ensure_active()?;
                let Operation::Write(p, r) = op else {
                    return Err(Error::permission_denied("reducer writes only"));
                };
                let input: InputEnvelope =
                    from_value(r.into_value(&structfs_core_store::NoCodec)?)?;
                let snapshot = client
                    .open(Command::Snapshot {
                        prefix: "".into(),
                        limits: ReadLimits::default(),
                    })
                    .await
                    .map_err(|e| Error::store("screen", "snapshot", e.to_string()))?;
                let projection = snapshot
                    .projection(65536, 128)
                    .await
                    .map_err(|e| Error::store("screen", "projection", e.to_string()))?;
                snapshot
                    .release()
                    .await
                    .map_err(|e| Error::store("screen", "release", e.to_string()))?;
                let count = match projection.root().and_then(|v| v.get(&path!("counter"))) {
                    Some(Value::Integer(n)) => *n as u64,
                    Some(Value::Unsigned(n)) => *n,
                    _ => 0,
                };
                let token = projection.token;
                // Pending state commits before even an immediately-ready catalog effect.
                let committed = client
                    .batch(
                        Some(token),
                        vec![
                            Mutation::Set {
                                path: "counter".into(),
                                value: Value::Unsigned(count + 1),
                            },
                            Mutation::Set {
                                path: "pending".into(),
                                value: Value::Bool(true),
                            },
                        ],
                    )
                    .await
                    .map_err(|e| Error::store("screen", "commit", e.to_string()))?;
                assert_eq!(
                    client.data(&path!("pending")).await.unwrap(),
                    Some(Value::Bool(true))
                );
                client
                    .batch(
                        Some(committed),
                        vec![Mutation::Set {
                            path: "pending".into(),
                            value: Value::Bool(false),
                        }],
                    )
                    .await
                    .map_err(|e| Error::store("screen", "catalog", e.to_string()))?;
                session.processed(input.sequence)?;
                Ok(Response::Written(p))
            })
        }
    }
    fn guest() -> Vec<u8> {
        br#"(module
  (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
  (import "structfs" "write" (func $write (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "block_alloc") (param i32) (result i32) (i32.const 32768))
  (data (i32.const 32) "{\22serialization\22:\22application/json\22}")
  (data (i32.const 256) "session/input/next") (data (i32.const 300) "reduce")
  (func (export "manifest") (param $r i32) (result i32)
    (i32.store (local.get $r) (i32.const 32)) (i32.store offset=4 (local.get $r) (i32.const 36)) (i32.const 0))
  (func $step
    (if (call $read (i32.const 256) (i32.const 18) (i32.const 1024)) (then unreachable))
    (if (call $write (i32.const 300) (i32.const 6) (i32.load (i32.const 1024)) (i32.load (i32.const 1028)) (i32.const 1032)) (then unreachable)))
  (func (export "run") (result i32) (call $step) (call $step) (i32.const 0)))"#.to_vec()
    }
    #[tokio::test(flavor = "current_thread")]
    async fn equal_labels_ordered_input_native_and_guest_have_identical_projections() {
        let supervisor = CleanupSupervisor::new(4).unwrap();
        let host = Arc::new(HeadlessHost::default());
        let budget = CallBudget::new(CallLimits::default());
        let mut owners = vec![];
        let mut projections = vec![];
        let mut sessions = vec![];
        for (index, surface) in ["left", "right"].iter().enumerate() {
            let owner = supervisor
                .owner(OwnerLimits {
                    retained_bytes: 32 << 20,
                    ..Default::default()
                })
                .unwrap();
            let session = host.open(&owner.handle(), surface, 2, 1024).unwrap();
            let state = State::new(
                &owner.handle(),
                Some(Value::Map(BTreeMap::from([
                    ("label".into(), Value::String("Catalog".into())),
                    ("credential_present".into(), Value::Bool(true)),
                ]))),
                StateLimits {
                    history_records: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            let raw = client(state.view(path!(""), true), &owner.handle(), budget.clone());
            let sc = StateClient::new(raw.clone());
            let observation = sc
                .open(Command::Observe {
                    prefix: "".into(),
                    limits: ReadLimits::default(),
                })
                .await
                .unwrap();
            let initial = observation.describe().await.unwrap().token;
            let reducer = Arc::new(Reducer {
                client: sc.clone(),
                session: session.clone(),
            });
            for sequence in 1..=2 {
                session
                    .submit(InputEnvelope {
                        version: 1,
                        session: session.status().session,
                        sequence,
                        input: Input::Key { text: "x".into() },
                    })
                    .unwrap();
            }
            assert!(session
                .submit(InputEnvelope {
                    version: 1,
                    session: session.status().session,
                    sequence: 3,
                    input: Input::Key { text: "x".into() }
                })
                .is_err());
            assert_eq!(session.status().accepted, 2);
            if index == 0 {
                let reducer_client = client(reducer, &owner.handle(), budget.clone());
                for _ in 0..2 {
                    let input = session.next(&CancelToken::new()).await.unwrap();
                    reducer_client
                        .write(&path!(""), Record::parsed(to_value(&input).unwrap()))
                        .await
                        .unwrap();
                }
            } else {
                let engine = CoreWasmEngine::with_limits(1, 1, 65536).unwrap();
                let code = Arc::new(engine.prepare(guest()).await.unwrap());
                let mut runtime = Runtime::new();
                runtime.register_core_artifact("screen", code);
                let profiled = Profiled::new(
                    session.clone(),
                    vec![Declaration {
                        profile: Profile::Interactive,
                        version: 1,
                        implementation: Implementation::Reference,
                    }],
                )
                .unwrap();
                let def=AssemblyDef::from_str(r#"{"assembly":"screen","imports":{"session":"input","reduce":"reducer"},"blocks":{"ui":"screen"},"public":"ui","wiring":["ui:/session -> $session","ui:/reduce -> $reduce"]}"#).unwrap();
                let instance = runtime
                    .instantiate(
                        &def,
                        HashMap::from([
                            ("session".into(), service_host_store(profiled)),
                            ("reduce".into(), service_host_store(reducer)),
                        ]),
                        ".".as_ref(),
                    )
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(2), instance.wait_public_terminal())
                    .await
                    .unwrap();
                assert_eq!(instance.public_cell().state(), BlockState::Stopped);
                assert!(instance.shutdown(Duration::from_secs(1)).await.complete());
            }
            assert_eq!(session.status().processed, 2);
            assert!(session.status().rendered.is_none());
            assert!(matches!(
                observation.changes(&initial).await,
                Err(ClientError::State(Fault::CursorExpired { .. }))
            ));
            let resync = sc
                .open(Command::Observe {
                    prefix: "".into(),
                    limits: ReadLimits::default(),
                })
                .await
                .unwrap();
            let projection = resync.projection(65536, 128).await.unwrap();
            assert!(projection
                .root()
                .unwrap()
                .get(&path!("credential"))
                .is_none());
            session.presented(projection.token.clone()).unwrap();
            projections.push(projection.root().cloned());
            sessions.push(session);
            owners.push(owner);
        }
        assert_eq!(projections[0], projections[1]);
        assert!(owners[0].close(Duration::from_secs(1)).await.is_quiescent());
        assert!(!sessions[1].status().closed);
        sessions[1]
            .submit(InputEnvelope {
                version: 1,
                session: sessions[1].status().session,
                sequence: 3,
                input: Input::Key {
                    text: "peer still live".into(),
                },
            })
            .unwrap();
        assert!(owners[1].close(Duration::from_secs(1)).await.is_quiescent());
        assert_eq!(budget.usage().calls, 0);
    }
    #[tokio::test]
    async fn superseded_catalog_ignoring_cancellation_cannot_publish_stale_results() {
        let supervisor = CleanupSupervisor::new(2).unwrap();
        let owner = supervisor.owner(Default::default()).unwrap();
        let request = supervisor.owner(Default::default()).unwrap();
        let state = State::new(&owner.handle(), Some(Value::Null), StateLimits::default()).unwrap();
        let c = StateClient::new(client(
            state.view(path!(""), true),
            &owner.handle(),
            CallBudget::new(CallLimits::default()),
        ));
        let token = c
            .batch(
                None,
                vec![Mutation::Set {
                    path: "".into(),
                    value: Value::Map(BTreeMap::new()),
                }],
            )
            .await
            .unwrap();
        let (send, recv) = tokio::sync::oneshot::channel();
        let stale = c.clone();
        let (done, result) = tokio::sync::oneshot::channel();
        request
            .handle()
            .spawn(move |_| async move {
                recv.await.unwrap();
                let r = stale
                    .batch(
                        Some(token),
                        vec![Mutation::Set {
                            path: "result".into(),
                            value: "obsolete".into(),
                        }],
                    )
                    .await;
                done.send(matches!(r, Err(ClientError::State(Fault::Conflict { .. }))))
                    .unwrap();
                Ok(())
            })
            .unwrap();
        c.batch(
            None,
            vec![Mutation::Set {
                path: "generation".into(),
                value: 2i64.into(),
            }],
        )
        .await
        .unwrap();
        request.cancel();
        send.send(()).unwrap();
        assert!(result.await.unwrap());
        assert!(request.close(Duration::from_secs(1)).await.is_quiescent());
        assert_eq!(c.data(&path!("result")).await.unwrap(), None);
    }
}
