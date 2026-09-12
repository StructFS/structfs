use structfs_profiles::*;
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
    use structfs_service::{CancelToken, CleanupSupervisor, OwnerLimits};
    let supervisor = CleanupSupervisor::new(2).unwrap();
    let owner = supervisor.owner(OwnerLimits::default()).unwrap();
    let other = supervisor.owner(OwnerLimits::default()).unwrap();
    let host = Arc::new(HeadlessHost::default());
    let a = host.open(&owner.handle(), "surface", 1, 1024).unwrap();
    assert!(host.open(&other.handle(), "surface", 1, 1024).is_err());
    let b = host.open(&other.handle(), "other", 1, 1024).unwrap();
    let input = InputEnvelope {
        version: 1,
        session: a.status().session,
        sequence: 1,
        input: Input::Key { text: "x".into() },
    };
    a.submit(input.clone()).unwrap();
    assert!(a
        .submit(InputEnvelope {
            sequence: 2,
            ..input
        })
        .is_err());
    assert_eq!(a.status().accepted, 1);
    let event = a.next(&CancelToken::new()).await.unwrap();
    assert_eq!(event.sequence, 1);
    assert_eq!(a.status().processed, 0);
    assert!(a.processed(2).is_err());
    a.processed(1).unwrap();
    assert!(a.status().rendered.is_none());
    a.presented(Token {
        epoch: "e".into(),
        revision: 2,
    })
    .unwrap();
    assert!(a
        .presented(Token {
            epoch: "e".into(),
            revision: 1
        })
        .is_err());
    assert!(owner.close(Duration::from_secs(1)).await.is_quiescent());
    assert!(!b.status().closed);
    let replacement = host.open(&other.handle(), "surface", 1, 1024).unwrap();
    a.release();
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
    let declaration = Declaration {
        profile: Profile::Interactive,
        version: 1,
        implementation: Implementation::Reference,
    };
    assert!(Profiled::new(
        inner.clone(),
        vec![Declaration {
            version: 2,
            ..declaration.clone()
        }]
    )
    .is_err());
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
    let mut ack = CommitAck {
        token: Token {
            epoch: "e".into(),
            revision: 1,
        },
        persisted: false,
        durability: Durability::Memory,
    };
    ack.validate().unwrap();
    ack.persisted = true;
    assert!(ack.validate().is_err());
    ack.durability = Durability::FileSynced;
    ack.validate().unwrap();
}
