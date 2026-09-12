//! Persistent conversation service, fresh guest turns, explicit approval, and a
//! small fsync-backed fixture journal. This is not a production ledger adapter.
#[cfg(test)]
mod tests {
    use featherweight_runtime::{
        host_store, service_host_store, AssemblyDef, BlockState, CoreWasmEngine, Runtime,
    };
    use std::{
        collections::{BTreeMap, HashMap},
        fs::{File, OpenOptions},
        io::{Read, Write},
        path::PathBuf,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
    };
    use structfs_core_store::{path, DetachedFuture, Error, MemoryStore, ReadOnly, Value};
    use structfs_profiles::{
        Approval, CommitAck, Declaration, Durability, Implementation, ProcessRequest, Profile,
        Profiled,
    };
    use structfs_serde_store::{from_value, to_value};
    use structfs_service::*;
    use structfs_state::{
        ClientError, Command, Fault, ReadLimits, State, StateClient, StateLimits,
    };
    struct JournalState {
        file: Option<File>,
        records: BTreeMap<String, String>,
    }
    struct Journal {
        state: Arc<Mutex<JournalState>>,
        path: PathBuf,
        _registration: Registration,
    }
    impl Journal {
        fn open(owner: &OwnerHandle, path: PathBuf) -> Arc<Self> {
            let mut file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&path)
                .unwrap();
            let mut text = String::new();
            file.read_to_string(&mut text).unwrap();
            let records = text
                .lines()
                .map(|line| serde_json::from_str::<(String, String)>(line).unwrap())
                .collect();
            let state = Arc::new(Mutex::new(JournalState {
                file: Some(file),
                records,
            }));
            let cleanup = state.clone();
            let registration = owner
                .register(ResourceKind::Registration, 4096, move || async move {
                    tokio::task::spawn_blocking(move || {
                        cleanup.lock().unwrap().file.take();
                    })
                    .await
                    .map_err(|e| Error::store("journal", "close", e.to_string()))?;
                    Ok(())
                })
                .unwrap();
            Arc::new(Self {
                state,
                path,
                _registration: registration,
            })
        }
        fn commit(&self, id: &str, payload: &str) -> Result<CommitAck, Error> {
            let mut s = self.state.lock().unwrap();
            if s.file.is_none() {
                return Err(Error::cancelled("journal closed"));
            }
            if let Some(old) = s.records.get(id) {
                if old != payload {
                    return Err(Error::conflict(
                        "operation identity reused for different content",
                    ));
                }
            } else {
                if s.records.len() >= 16 || payload.len() > 1024 || id.len() > 128 {
                    return Err(Error::overloaded("fixture journal bounds"));
                }
                let mut line = serde_json::to_vec(&(id, payload)).unwrap();
                line.push(b'\n');
                let file = s.file.as_mut().unwrap();
                file.write_all(&line)?;
                file.sync_all()?;
                s.records.insert(id.into(), payload.into());
            }
            Ok(CommitAck {
                token: structfs_state::Token {
                    epoch: self.path.file_name().unwrap().to_string_lossy().into(),
                    revision: s.records.len() as u64,
                },
                persisted: true,
                durability: Durability::FileSynced,
            })
        }
    }
    struct Tool {
        id: String,
        journal: Arc<Journal>,
        events: Arc<OwnedTail>,
        approval: Arc<Mutex<Option<bool>>>,
        notify: Arc<tokio::sync::Notify>,
        started: Arc<tokio::sync::Notify>,
        cancelled: Arc<AtomicUsize>,
        executions: Arc<AtomicUsize>,
    }
    impl Tool {
        fn approve(&self, a: Approval) -> Result<(), Error> {
            if a.version != 1 || a.operation != self.id {
                return Err(Error::conflict("approval operation identity"));
            }
            let mut slot = self.approval.lock().unwrap();
            if slot.is_some() {
                return Err(Error::conflict("approval already decided"));
            }
            *slot = Some(a.approved);
            self.notify.notify_waiters();
            Ok(())
        }
    }
    struct Pending {
        cancelled: Arc<AtomicUsize>,
        completed: bool,
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            if !self.completed {
                self.cancelled.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    impl Service for Tool {
        fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
            let expected = self.id.clone();
            let journal = self.journal.clone();
            let events = self.events.clone();
            let approval = self.approval.clone();
            let notify = self.notify.clone();
            let started = self.started.clone();
            let cancelled = self.cancelled.clone();
            let executions = self.executions.clone();
            Box::pin(async move {
                let Operation::Write(p, r) = op else {
                    return Err(Error::permission_denied("process start requires write"));
                };
                let request: ProcessRequest =
                    from_value(r.into_value(&structfs_core_store::NoCodec)?)?;
                if request.version != 1
                    || request.operation != expected
                    || request.program != "fake-tool"
                    || request.environment_grant != "safe_env"
                    || request.workspace_grant != "workspace"
                {
                    return Err(Error::permission_denied("process grant"));
                }
                // Explicit application idempotency: a completed operation is not reexecuted.
                if journal
                    .state
                    .lock()
                    .unwrap()
                    .records
                    .contains_key(&expected)
                {
                    return Ok(Response::Written(p));
                }
                let mut pending = Pending {
                    cancelled,
                    completed: false,
                };
                started.notify_one();
                loop {
                    let wake = notify.notified();
                    tokio::pin!(wake);
                    wake.as_mut().enable();
                    if let Some(approved) = *approval.lock().unwrap() {
                        if !approved {
                            return Err(Error::permission_denied("approval denied"));
                        }
                        break;
                    }
                    tokio::select! {_=c.cancellation.cancelled()=>return Err(Error::cancelled("tool cancelled")),_=wake=>{}}
                }
                executions.fetch_add(1, Ordering::SeqCst);
                let id = expected.clone();
                let lease = c.lease();
                let ack = tokio::task::spawn_blocking(move || {
                    let _lease = lease;
                    journal.commit(&id, "tool result")
                })
                .await
                .map_err(|e| Error::store("journal", "commit", e.to_string()))??;
                assert!(ack.persisted);
                assert_eq!(ack.durability, Durability::FileSynced);
                // Publication follows the fsync acknowledgment. A full observer tail
                // cannot turn this already committed operation into an uncommitted one.
                let event = serde_json::to_vec(&(expected, ack)).unwrap();
                let _ = events.push(event);
                pending.completed = true;
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
 (data (i32.const 256) "request") (data (i32.const 300) "tool/start")
 (func (export "manifest") (param $r i32) (result i32) (i32.store (local.get $r) (i32.const 32)) (i32.store offset=4 (local.get $r) (i32.const 36)) (i32.const 0))
 (func (export "run") (result i32)
  (if (call $read (i32.const 256) (i32.const 7) (i32.const 1024)) (then unreachable))
  (if (call $write (i32.const 300) (i32.const 10) (i32.load (i32.const 1024)) (i32.load (i32.const 1028)) (i32.const 1032)) (then unreachable)) (i32.const 0)))"#.to_vec()
    }
    #[tokio::test]
    async fn approvals_observer_disconnect_turn_cancel_restart_and_durable_ack() {
        let temp = std::env::temp_dir().join(format!(
            "structfs-conversation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let supervisor = CleanupSupervisor::new(4).unwrap();
        let service = supervisor
            .owner(OwnerLimits {
                retained_bytes: 32 << 20,
                ..Default::default()
            })
            .unwrap();
        let journal = Journal::open(&service.handle(), temp.clone());
        let events = Arc::new(OwnedTail::new(&service.handle(), 2, 2048).unwrap());
        let executions = Arc::new(AtomicUsize::new(0));
        let cancelled = Arc::new(AtomicUsize::new(0));
        let state = State::new(
            &service.handle(),
            Some(Value::Map(BTreeMap::new())),
            StateLimits::default(),
        )
        .unwrap();
        let old_epoch = state.token();
        let engine = CoreWasmEngine::with_limits(3, 3, 65536).unwrap();
        let prepared = Arc::new(engine.prepare(guest()).await.unwrap());
        for id in ["approved", "cancelled", "approved", "shutdown"] {
            let tool = Arc::new(Tool {
                id: id.into(),
                journal: journal.clone(),
                events: events.clone(),
                approval: Arc::new(Mutex::new(None)),
                notify: Arc::new(tokio::sync::Notify::new()),
                started: Arc::new(tokio::sync::Notify::new()),
                cancelled: cancelled.clone(),
                executions: executions.clone(),
            });
            let mut runtime = Runtime::new();
            runtime.register_core_artifact("turn", prepared.clone());
            let request = ProcessRequest {
                version: 1,
                operation: id.into(),
                program: "fake-tool".into(),
                args: vec![],
                environment_grant: "safe_env".into(),
                workspace_grant: "workspace".into(),
            };
            let declarations = vec![Declaration {
                profile: Profile::Process,
                version: 1,
                implementation: Implementation::FixtureOnly,
            }];
            let provider =
                Profiled::new(service.handle().service(tool.clone()), declarations).unwrap();
            let def=AssemblyDef::from_str(r#"{"assembly":"turn","imports":{"request":"input","tool":"approved capability"},"blocks":{"turn":"turn"},"public":"turn","wiring":["turn:/request -> $request","turn:/tool -> $tool"]}"#).unwrap();
            let instance = runtime
                .instantiate(
                    &def,
                    HashMap::from([
                        (
                            "request".into(),
                            host_store(ReadOnly::new(MemoryStore::with_root(
                                to_value(&request).unwrap(),
                            ))),
                        ),
                        ("tool".into(), service_host_store(provider)),
                    ]),
                    ".".as_ref(),
                )
                .unwrap();
            let duplicate = id == "approved" && executions.load(Ordering::SeqCst) > 0;
            if !duplicate {
                tokio::time::timeout(Duration::from_secs(2), tool.started.notified())
                    .await
                    .unwrap();
                assert!(tool
                    .approve(Approval {
                        version: 1,
                        operation: "wrong".into(),
                        approved: true
                    })
                    .is_err());
                if id == "approved" {
                    let observer = CancelToken::new();
                    observer.cancel();
                    assert!(events.read(0, 1, &observer).await.is_err());
                    assert_eq!(instance.public_cell().state(), BlockState::Running);
                    tool.approve(Approval {
                        version: 1,
                        operation: id.into(),
                        approved: true,
                    })
                    .unwrap();
                } else if id == "cancelled" {
                    assert!(instance.shutdown(Duration::ZERO).await.complete());
                } else {
                    assert!(service.close(Duration::from_secs(1)).await.is_quiescent());
                }
            }
            tokio::time::timeout(Duration::from_secs(2), instance.wait_public_terminal())
                .await
                .unwrap();
            assert!(instance.shutdown(Duration::from_secs(1)).await.complete());
            if id == "approved" {
                let page = events.read(0, 1, &CancelToken::new()).await.unwrap();
                let (_id, ack): (String, CommitAck) =
                    serde_json::from_slice(&page.items[0]).unwrap();
                assert!(ack.persisted);
                assert!(std::fs::read_to_string(&temp).unwrap().contains("approved"));
            }
        }
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        assert_eq!(cancelled.load(Ordering::SeqCst), 2);
        let restarted = supervisor
            .owner(OwnerLimits {
                retained_bytes: 32 << 20,
                ..Default::default()
            })
            .unwrap();
        let reopened = Journal::open(&restarted.handle(), temp.clone());
        let j = reopened.clone();
        let ack = tokio::task::spawn_blocking(move || j.commit("approved", "tool result"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ack.token.revision, 1);
        let j = reopened.clone();
        assert!(
            tokio::task::spawn_blocking(move || j.commit("approved", "different result"))
                .await
                .unwrap()
                .is_err()
        );
        // Configuration persistence is an explicit boundary, distinct from state snapshots.
        let j = reopened.clone();
        let config = tokio::task::spawn_blocking(move || j.commit("configuration", "saved"))
            .await
            .unwrap()
            .unwrap();
        assert!(config.persisted);
        assert_eq!(config.token.revision, 2);
        let fresh = State::new(
            &restarted.handle(),
            Some(Value::Null),
            StateLimits::default(),
        )
        .unwrap();
        let raw = Router::new(vec![Mount::new(
            path!(""),
            path!(""),
            fresh.view(path!(""), true),
            Arc::new(BudgetAdmission {
                budget: CallBudget::<String>::new(CallLimits::default()),
                key: "state".into(),
            }),
        )])
        .unwrap()
        .client();
        assert!(matches!(
            StateClient::new(raw)
                .open(Command::Watch {
                    prefix: "".into(),
                    after: old_epoch,
                    limits: ReadLimits::default()
                })
                .await,
            Err(ClientError::State(Fault::EpochMismatch { .. }))
        ));
        assert!(restarted.close(Duration::from_secs(1)).await.is_quiescent());
        drop(journal);
        drop(reopened);
        std::fs::remove_file(temp).unwrap();
    }
}
