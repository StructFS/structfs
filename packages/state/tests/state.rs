#![cfg(feature = "service")]
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use structfs_core_store::{path, Record, Value};
use structfs_service::*;
use structfs_state::*;
fn setup(limits: StateLimits) -> (CleanupSupervisor, Owner, Arc<State>, StateClient, Client) {
    let supervisor = CleanupSupervisor::new(8).unwrap();
    let owner = supervisor
        .owner(OwnerLimits {
            resources: 512,
            retained_bytes: 64 << 20,
        })
        .unwrap();
    let state = State::new(&owner.handle(), Some(Value::Map(BTreeMap::new())), limits).unwrap();
    let client = route(state.view(path!(""), true));
    (
        supervisor,
        owner,
        state,
        StateClient::new(client.clone()),
        client,
    )
}
fn route(service: Arc<dyn Service>) -> Client {
    Router::new(vec![Mount::new(
        path!(""),
        path!(""),
        service,
        Arc::new(BudgetAdmission {
            budget: CallBudget::<String>::new(CallLimits::default()),
            key: "state".into(),
        }),
    )])
    .unwrap()
    .client()
}
fn set(p: &str, v: Value) -> Mutation {
    Mutation::Set {
        path: p.into(),
        value: v,
    }
}
fn reads() -> ReadLimits {
    ReadLimits {
        page_items: 1,
        ..Default::default()
    }
}
#[tokio::test]
async fn atomic_batches_conflicts_null_missing_and_occurrences() {
    let (_supervisor, owner, state, client, raw) = setup(StateLimits::default());
    let initial = state.token();
    let t = client
        .batch(
            Some(initial.clone()),
            vec![set("a", Value::Null), set("b", Value::Unsigned(u64::MAX))],
        )
        .await
        .unwrap();
    assert_eq!(t.revision, 1);
    assert_eq!(client.data(&path!("a")).await.unwrap(), Some(Value::Null));
    assert_eq!(client.data(&path!("missing")).await.unwrap(), None);
    assert!(matches!(
        client
            .batch(Some(initial), vec![set("b", 0i64.into())])
            .await,
        Err(ClientError::State(Fault::Conflict { .. }))
    ));
    assert!(client
        .batch(None, vec![set("c", 1i64.into()), set("b/x", 2i64.into())])
        .await
        .is_err());
    assert_eq!(client.data(&path!("c")).await.unwrap(), None);
    assert_eq!(state.token(), t);
    assert!(client.batch(None, vec![]).await.is_err());
    let again = client
        .batch(None, vec![set("a", Value::Null)])
        .await
        .unwrap();
    assert_eq!(again.revision, 2);
    assert!(raw
        .write(&path!("data/a"), Record::parsed(1i64.into()))
        .await
        .is_err());
    client
        .batch(None, vec![Mutation::Delete { path: "".into() }])
        .await
        .unwrap();
    assert_eq!(client.data(&path!("")).await.unwrap(), None);
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
}
#[tokio::test]
async fn observe_pins_snapshot_without_a_subscription_gap() {
    let (_s, owner, state, c, _) = setup(StateLimits::default());
    c.batch(
        None,
        vec![
            set("tree/x", 1i64.into()),
            set("tree/empty", Value::Array(vec![])),
        ],
    )
    .await
    .unwrap();
    let h = c
        .open(Command::Observe {
            prefix: "tree".into(),
            limits: reads(),
        })
        .await
        .unwrap();
    let start = h.describe().await.unwrap().token;
    c.batch(None, vec![set("tree/x", 2i64.into())])
        .await
        .unwrap();
    let mut offset = 0;
    let mut nodes = vec![];
    loop {
        let page = h.snapshot(offset).await.unwrap();
        assert_eq!(page.token, start);
        offset = page.next;
        nodes.extend(page.items);
        if page.done {
            break;
        }
    }
    assert!(nodes
        .iter()
        .any(|n| n.path == vec!["x"] && n.value == Value::Integer(1)));
    assert!(nodes
        .iter()
        .any(|n| n.path == vec!["empty"] && n.value == Value::Array(vec![])));
    let changes = h.changes(&start).await.unwrap();
    assert_eq!(changes.items.len(), 1);
    assert_eq!(changes.next, state.token());
    assert_eq!(changes.items[0].paths, vec![""]);
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    assert!(h.snapshot(0).await.is_err());
}
#[tokio::test]
async fn expiry_future_epoch_filtering_and_atomic_history_pages() {
    let limits = StateLimits {
        history_records: 2,
        ..Default::default()
    };
    let (_s, _o, state, c, _) = setup(limits);
    let start = state.token();
    let h = c
        .open(Command::Watch {
            prefix: "a".into(),
            after: start.clone(),
            limits: reads(),
        })
        .await
        .unwrap();
    c.batch(None, vec![set("b/x", 1i64.into())]).await.unwrap();
    let empty = h.changes(&start).await.unwrap();
    assert!(empty.items.is_empty());
    assert_eq!(empty.next.revision, 1);
    c.batch(None, vec![set("a/x", 1i64.into()), set("a/y", 2i64.into())])
        .await
        .unwrap();
    let p = h.changes(&empty.next).await.unwrap();
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.next.revision, 2);
    c.batch(None, vec![set("a/z", 3i64.into())]).await.unwrap();
    assert!(
        matches!(h.changes(&start).await,Err(ClientError::State(Fault::CursorExpired{earliest})) if earliest.revision==1)
    );
    let mut future = state.token();
    future.revision += 1;
    assert!(matches!(
        h.changes(&future).await,
        Err(ClientError::State(Fault::Invalid { .. }))
    ));
    let mut other = state.token();
    other.epoch = "different".into();
    assert!(matches!(
        c.open(Command::Watch {
            prefix: "".into(),
            after: other,
            limits: reads()
        })
        .await,
        Err(ClientError::State(Fault::EpochMismatch { .. }))
    ));
}
#[tokio::test]
async fn grants_confine_payload_paths_and_snapshot_keys_are_lossless() {
    let (_s, _o, state, c, _) = setup(StateLimits::default());
    c.batch(
        None,
        vec![
            set("secret", 99i64.into()),
            set(
                "tenant",
                Value::Map(BTreeMap::from([(
                    "not/a/path".into(),
                    Value::Bytes(vec![0, 255]),
                )])),
            ),
        ],
    )
    .await
    .unwrap();
    let scoped = StateClient::new(route(state.view(path!("tenant"), true)));
    scoped
        .batch(None, vec![set("secret", 1i64.into())])
        .await
        .unwrap();
    assert_eq!(
        c.data(&path!("secret")).await.unwrap(),
        Some(Value::Integer(99))
    );
    assert!(scoped
        .batch(None, vec![set("../secret", 0i64.into())])
        .await
        .is_err());
    let h = scoped
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: ReadLimits::default(),
        })
        .await
        .unwrap();
    let p = h.snapshot(0).await.unwrap();
    assert!(p
        .items
        .iter()
        .any(|n| n.path == vec!["not/a/path"] && n.value == Value::Bytes(vec![0, 255])));
    let read_only = StateClient::new(route(state.view(path!("tenant"), false)));
    assert!(read_only
        .batch(None, vec![set("", Value::Null)])
        .await
        .is_err());
}
#[tokio::test]
async fn handle_limits_request_cleanup_and_parked_read_cancellation() {
    let (s, _o, state, c, raw) = setup(StateLimits {
        handles: 1,
        ..Default::default()
    });
    let request = s.owner(OwnerLimits::default()).unwrap();
    let scoped = StateClient::new(raw.owned_by(&request.handle()));
    let h = scoped
        .open(Command::Observe {
            prefix: "".into(),
            limits: reads(),
        })
        .await
        .unwrap();
    let token = state.token();
    assert!(c
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: reads()
        })
        .await
        .is_err());
    let parked = tokio::spawn(async move { h.changes(&token).await });
    assert!(request.close(Duration::from_secs(1)).await.is_quiescent());
    assert!(parked.await.unwrap().is_err());
    assert!(c
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: reads()
        })
        .await
        .is_ok());
}
#[tokio::test]
async fn size_and_page_rejections_leave_state_unchanged() {
    let (_s, _o, state, c, _) = setup(StateLimits {
        change_bytes: 512,
        ..Default::default()
    });
    let t = state.token();
    let mutations = (0..20)
        .map(|i| set(&format!("key{i}/x"), 1i64.into()))
        .collect();
    assert!(matches!(
        c.batch(None, mutations).await,
        Err(ClientError::State(Fault::ResourceLimit { .. }))
    ));
    assert_eq!(state.token(), t);
    assert!(c
        .open(Command::Observe {
            prefix: "".into(),
            limits: ReadLimits {
                page_bytes: 128,
                page_items: 1
            }
        })
        .await
        .is_err());
    c.batch(None, vec![set("big", Value::String("x".repeat(70000)))])
        .await
        .unwrap();
    assert!(matches!(
        c.open(Command::Snapshot {
            prefix: "".into(),
            limits: ReadLimits::default()
        })
        .await,
        Err(ClientError::State(Fault::ResourceLimit { .. }))
    ));
}

#[tokio::test(start_paused = true)]
async fn handle_age_expires_even_a_parked_watch_without_commits() {
    let (_s, _o, state, c, _) = setup(StateLimits {
        handles: 1,
        handle_age: Duration::from_secs(2),
        ..Default::default()
    });
    let start = state.token();
    let h = c
        .open(Command::Watch {
            prefix: "".into(),
            after: start.clone(),
            limits: reads(),
        })
        .await
        .unwrap();
    let read = tokio::spawn(async move { h.changes(&start).await });
    tokio::time::advance(Duration::from_secs(3)).await;
    assert!(matches!(
        read.await.unwrap(),
        Err(ClientError::State(Fault::Closed))
    ));
    assert!(c
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: reads()
        })
        .await
        .is_ok());
}
#[tokio::test]
async fn receipt_admission_failure_cannot_commit_and_projection_is_immutable() {
    use structfs_core_store::Reader;
    let (s, _o, state, c, raw) = setup(StateLimits::default());
    let request = s
        .owner(OwnerLimits {
            resources: 1,
            retained_bytes: 65536,
        })
        .unwrap();
    let denied = StateClient::new(raw.owned_by(&request.handle()));
    let before = state.token();
    assert!(denied
        .batch(None, vec![set("x", 1i64.into())])
        .await
        .is_err());
    assert_eq!(state.token(), before);
    c.batch(None, vec![set("x", Value::Unsigned(u64::MAX))])
        .await
        .unwrap();
    let h = c
        .open(Command::Snapshot {
            prefix: "".into(),
            limits: reads(),
        })
        .await
        .unwrap();
    let mut projection = h.projection(1 << 20, 100).await.unwrap();
    c.batch(None, vec![set("x", 2i64.into())]).await.unwrap();
    h.release().await.unwrap();
    assert_eq!(
        projection.read(&path!("x")).unwrap().unwrap().as_value(),
        Some(&Value::Unsigned(u64::MAX))
    );
    assert_eq!(projection.token.revision, 1);
}
#[tokio::test]
async fn concurrent_conditional_batches_have_one_winner_and_no_partial_publication() {
    let (_s, _o, state, c, _) = setup(StateLimits::default());
    let token = state.token();
    let (a, b) = tokio::join!(
        c.batch(Some(token.clone()), vec![set("a/x", 1i64.into())]),
        c.batch(Some(token), vec![set("b/x", 1i64.into())])
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(state.token().revision, 1);
    let present = usize::from(c.data(&path!("a")).await.unwrap().is_some())
        + usize::from(c.data(&path!("b")).await.unwrap().is_some());
    assert_eq!(present, 1);
}
