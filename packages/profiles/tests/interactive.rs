use structfs_profiles::*;
#[test]
fn shared_input_corpus() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/input-v1.json")).unwrap();
    let mut last = 0;
    for case in cases {
        let input: InputEnvelope = serde_json::from_value(case["event"].clone()).unwrap();
        let valid = input.validate("s", last).is_ok();
        assert_eq!(valid, case["ok"].as_bool().unwrap());
        if valid {
            last = input.sequence;
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
