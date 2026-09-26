use structfs_profiles::*;
use structfs_state::Token;
#[test]
fn shared_input_corpus() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/input-v1.json")).unwrap();
    let mut last = 0;
    for case in cases {
        let parsed = serde_json::from_value::<InputEnvelope>(case["event"].clone());
        let valid = parsed
            .as_ref()
            .is_ok_and(|input| input.validate("s", last).is_ok());
        assert_eq!(valid, case["ok"].as_bool().unwrap(), "{case}");
        if valid {
            last = parsed.unwrap().sequence;
        }
    }
    assert_eq!(last, 6);
}
#[cfg(feature = "host")]
#[tokio::test(flavor = "current_thread")]
async fn queue_bounds_identity_and_presentation_are_independent() {
    use std::{sync::Arc, time::Duration};
    use structfs_handles::CancelToken;
    use structfs_service::{CleanupSupervisor, OwnerLimits};
    let supervisor = CleanupSupervisor::new(2).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let other = supervisor.owner(OwnerLimits::default()).unwrap();
    let host = Arc::new(HeadlessHost::default());
    let a = host.open(&owner.handle(), "surface", 1, 1024).unwrap();
    assert!(host.open(&other.handle(), "surface", 1, 1024).is_err());
    let b = host.open(&other.handle(), "other", 1, 1024).unwrap();
    let session = a.status().session;
    let key = Input::Key { text: "x".into() };
    a.submit(InputEnvelope::new(&session, 1, key.clone()))
        .unwrap();
    assert!(a.submit(InputEnvelope::new(&session, 2, key)).is_err());
    assert_eq!(a.status().accepted, 1);
    let event = a.next(&CancelToken::new()).await.unwrap();
    assert_eq!(event.sequence, 1);
    assert_eq!(a.status().processed, 0);
    assert!(a.processed(2).is_err());
    a.processed(1).unwrap();
    assert!(a.status().rendered.is_none());
    a.presented(Token::new("e", 2)).unwrap();
    assert!(a.presented(Token::new("e", 1)).is_err());
    assert!(owner.join(Duration::from_secs(1)).await.is_quiescent());
    assert!(!b.status().closed);
    let replacement = host.open(&other.handle(), "surface", 1, 1024).unwrap();
    a.close();
    assert!(!replacement.status().closed);
}

#[cfg(feature = "host")]
#[tokio::test]
async fn discovery_is_pure_read_only_and_version_checked() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use structfs_core_store::{path, DetachedFuture, Error, Record, Value};
    use structfs_service::*;
    struct Effects(Arc<AtomicUsize>);
    impl Service for Effects {
        fn call(&self, _: CallContext, _: Operation) -> DetachedFuture<Response> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(Error::permission_denied("effect")) })
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let inner = Arc::new(Effects(calls.clone()));
    let declaration = Declaration::new(Profile::Interactive, Implementation::Reference);
    let mut wrong_version = declaration.clone();
    wrong_version.version = 2;
    assert!(Profiled::new(inner.clone(), vec![wrong_version]).is_err());
    assert!(Profiled::new(
        inner.clone(),
        vec![declaration.clone(), declaration.clone()]
    )
    .is_err());
    let provider = Profiled::new(inner, vec![declaration]).unwrap();
    let response = provider
        .call(
            CallContext::default(),
            Operation::Read(path!("meta/profiles")),
        )
        .await
        .unwrap();
    let Response::Read(Some(record)) = response else {
        panic!("missing discovery")
    };
    let declarations: Vec<Declaration> =
        structfs_serde_store::from_value(record.into_value(&structfs_core_store::NoCodec).unwrap())
            .unwrap();
    assert_eq!(declarations[0].profile, Profile::Interactive);
    assert!(provider
        .call(
            CallContext::default(),
            Operation::Write(path!("meta/profiles"), Record::parsed(Value::Null))
        )
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[test]
fn persistence_acknowledgments_cannot_claim_memory_is_durable() {
    let mut ack = CommitAck::new(Token::new("e", 1), Durability::Memory);
    assert!(!ack.persisted);
    ack.validate().unwrap();
    ack.persisted = true;
    assert!(ack.validate().is_err());
    ack.durability = Durability::FileSynced;
    ack.validate().unwrap();
}

#[cfg(feature = "host")]
#[tokio::test]
async fn shared_service_client_preserves_session_acceptance_and_release() {
    use std::{sync::Arc, time::Duration};
    use structfs_core_store::{path, NoCodec, Record, Value};
    use structfs_serde_store::{from_value, to_value};
    use structfs_service::*;
    let supervisor = CleanupSupervisor::new(2).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let host = Arc::new(HeadlessHost::default());
    assert!(host.open(&owner.handle(), "invalid", 0, 1024).is_err());
    let session = Arc::new(host.open(&owner.handle(), "surface", 2, 1024).unwrap());
    let router = Router::shared(vec![Mount::new(
        path!(""),
        path!(""),
        session.clone(),
        Arc::new(BudgetAdmission::new(
            CallBudget::<String>::shared(CallLimits::default()),
            "session",
        )),
    )])
    .unwrap();
    let client = router.client();
    let initial: SessionStatus = from_value(
        client
            .read(&path!("status"))
            .await
            .unwrap()
            .unwrap()
            .into_value(&NoCodec)
            .unwrap(),
    )
    .unwrap();
    let event = InputEnvelope::new(initial.session, 1, Input::Key { text: "x".into() });
    client
        .write(&path!("input"), Record::parsed(to_value(&event).unwrap()))
        .await
        .unwrap();
    let delivered: InputEnvelope = from_value(
        client
            .read(&path!("input/next"))
            .await
            .unwrap()
            .unwrap()
            .into_value(&NoCodec)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(delivered.sequence, 1);
    assert!(client.read(&path!("input/next")).await.is_err());
    assert!(client
        .write(&path!("processed"), Record::parsed(Value::Unsigned(2)))
        .await
        .is_err());
    client
        .write(&path!("processed"), Record::parsed(Value::Unsigned(1)))
        .await
        .unwrap();
    let token = Token::new("e", 1);
    client
        .write(
            &path!("presented"),
            Record::parsed(to_value(&token).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(session.status().processed, 1);
    assert_eq!(session.status().rendered, Some(token));
    assert!(client.read(&path!("unknown")).await.is_err());
    assert!(client
        .write(&path!("unknown"), Record::parsed(Value::Null))
        .await
        .is_err());
    assert!(client
        .write(&path!("release"), Record::parsed(Value::Bool(true)))
        .await
        .is_err());
    assert!(session.presented(Token::new("x".repeat(129), 2)).is_err());
    assert!(session.presented(Token::new("different", 2)).is_err());
    let pending_path = path!("input/next");
    let pending = client.read(&pending_path);
    tokio::pin!(pending);
    // Poll exactly once: a wall-clock timeout would pass or fail on
    // scheduler luck rather than on whether the read actually parked.
    std::future::poll_fn(|cx| {
        let polled = std::future::Future::poll(std::pin::Pin::new(&mut pending), cx);
        assert!(polled.is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    client
        .write(&path!("release"), Record::parsed(Value::Null))
        .await
        .unwrap();
    assert!(pending.await.is_err());
    assert!(session.presented(Token::new("e", 2)).is_err());
    assert!(owner.join(Duration::from_secs(1)).await.is_quiescent());
}
