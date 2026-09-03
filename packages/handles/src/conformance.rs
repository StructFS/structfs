//! Conformance checks for the handle-store protocol.
//!
//! Run these from a handle store's test suite to certify it follows the
//! `outstanding/{id}` rules — whether it's built on [`crate::HandleStore`]
//! or hand-rolled. Each check panics with a descriptive message on
//! violation.

use structfs_core_store::{DetachedStore, Error, Path, Record, Value};

fn root() -> Path {
    Path::parse("").unwrap()
}

/// Run every handle-protocol check against a fresh store.
///
/// `request` is a value the store accepts as a mint request.
pub async fn check_handle_conventions<S: DetachedStore>(store: &mut S, request: Value) {
    check_mint_returns_handle_path(store, request.clone()).await;
    check_overwrite_conflicts(store, request.clone()).await;
    check_release_and_absence(store, request.clone()).await;
    check_release_idempotent(store, request).await;
}

/// A root write mints a handle at `outstanding/{id}`.
pub async fn check_mint_returns_handle_path<S: DetachedStore>(store: &mut S, request: Value) {
    let path = store
        .write_detached(&root(), Record::parsed(request))
        .await
        .expect("mint write failed");
    assert!(
        path.len() == 2 && path[0] == "outstanding" && path[1].parse::<u64>().is_ok(),
        "mint must return outstanding/{{id}}, got: {}",
        path
    );
    let record = store
        .read_detached(&path)
        .await
        .expect("handle read failed");
    assert!(record.is_some(), "a live handle must be readable");
}

/// A non-Null write directly to a handle is a conflict.
pub async fn check_overwrite_conflicts<S: DetachedStore>(store: &mut S, request: Value) {
    let path = store
        .write_detached(&root(), Record::parsed(request))
        .await
        .expect("mint write failed");
    let err = store
        .write_detached(&path, Record::parsed(Value::from("clobber")))
        .await
        .expect_err("overwriting a live handle must fail");
    assert!(
        matches!(err, Error::Conflict { .. }),
        "handle overwrite must be Error::Conflict, got: {}",
        err
    );
}

/// A Null write releases the handle; released handles read as absent.
pub async fn check_release_and_absence<S: DetachedStore>(store: &mut S, request: Value) {
    let path = store
        .write_detached(&root(), Record::parsed(request))
        .await
        .expect("mint write failed");
    store
        .write_detached(&path, Record::parsed(Value::Null))
        .await
        .expect("Null write must release the handle");
    let record = store
        .read_detached(&path)
        .await
        .expect("post-release read failed");
    assert!(record.is_none(), "a released handle must read as absent");
}

/// Releasing twice, or releasing an unknown handle, is a no-op.
pub async fn check_release_idempotent<S: DetachedStore>(store: &mut S, request: Value) {
    let path = store
        .write_detached(&root(), Record::parsed(request))
        .await
        .expect("mint write failed");
    store
        .write_detached(&path, Record::parsed(Value::Null))
        .await
        .expect("first release failed");
    store
        .write_detached(&path, Record::parsed(Value::Null))
        .await
        .expect("double release must be a no-op");
    store
        .write_detached(
            &Path::parse("outstanding/18446744073709551614").unwrap(),
            Record::parsed(Value::Null),
        )
        .await
        .expect("releasing an unknown handle must be a no-op");
}
