//! Conformance checks for the StructFS store conventions.
//!
//! StructFS asserts several semantics by convention rather than by trait
//! signature. This module makes them checkable: call these functions from
//! a store's test suite to certify it instead of re-deriving the semantics
//! by hand.
//!
//! Each check panics with a descriptive message on violation, so calling
//! one from a `#[test]` is all a store's suite needs:
//!
//! ```rust
//! use structfs_core_store::{conformance, MemoryStore};
//!
//! conformance::check_conventions(&mut MemoryStore::new());
//! ```
//!
//! The checks write under top-level components prefixed `conformance_`;
//! run them against a fresh store instance.

use crate::{path, Path, Reader, Record, Store, Value};

/// Run every convention check against a fresh store.
pub fn check_conventions<S: Store>(store: &mut S) {
    check_leaf_roundtrip(store);
    check_missing_reads_none(store);
    check_deep_write_creates_intermediates(store);
    check_prefix_read_returns_children(store);
    check_read_children(store);
    check_null_write_deletes_subtree(store);
    check_map_write_replaces_subtree(store);
}

fn read_value<S: Reader>(store: &mut S, path: &Path) -> Option<Value> {
    store
        .read(path)
        .unwrap_or_else(|e| panic!("read {} failed: {}", path, e))
        .map(|record| {
            record
                .as_value()
                .unwrap_or_else(|| panic!("read {} returned a raw record", path))
                .clone()
        })
}

/// A leaf written at a path reads back equal.
pub fn check_leaf_roundtrip<S: Store>(store: &mut S) {
    let p = path!("conformance_leaf/value");
    store
        .write(&p, Record::parsed(Value::from("roundtrip")))
        .expect("leaf write failed");
    assert_eq!(
        read_value(store, &p),
        Some(Value::from("roundtrip")),
        "leaf write/read did not roundtrip"
    );
}

/// Reading a path that was never written returns `Ok(None)`.
pub fn check_missing_reads_none<S: Store>(store: &mut S) {
    assert_eq!(
        read_value(store, &path!("conformance_missing/never_written")),
        None,
        "missing path must read as None, not an error or a value"
    );
}

/// Writing a deep path creates intermediate maps.
pub fn check_deep_write_creates_intermediates<S: Store>(store: &mut S) {
    store
        .write(
            &path!("conformance_deep/a/b/c"),
            Record::parsed(Value::from(1i64)),
        )
        .expect("deep write must create intermediate maps");
    for prefix in [
        "conformance_deep",
        "conformance_deep/a",
        "conformance_deep/a/b",
    ] {
        let p = Path::parse(prefix).unwrap();
        assert!(
            matches!(read_value(store, &p), Some(Value::Map(_))),
            "intermediate {} must exist as a map after a deep write",
            prefix
        );
    }
}

/// Reading at a prefix returns a `Value::Map` containing the children.
pub fn check_prefix_read_returns_children<S: Store>(store: &mut S) {
    store
        .write(
            &path!("conformance_tree/users/alice"),
            Record::parsed(Value::from(1i64)),
        )
        .expect("write failed");
    store
        .write(
            &path!("conformance_tree/users/bob"),
            Record::parsed(Value::from(2i64)),
        )
        .expect("write failed");

    match read_value(store, &path!("conformance_tree/users")) {
        Some(Value::Map(map)) => {
            assert!(
                map.contains_key("alice") && map.contains_key("bob"),
                "prefix read must include children as map keys, got: {:?}",
                map.keys().collect::<Vec<_>>()
            );
        }
        other => panic!(
            "prefix read must return a map of children, got: {:?}",
            other
        ),
    }
}

/// `read_children` enumerates direct children at a prefix.
pub fn check_read_children<S: Store>(store: &mut S) {
    store
        .write(
            &path!("conformance_children/x"),
            Record::parsed(Value::from(1i64)),
        )
        .expect("write failed");
    store
        .write(
            &path!("conformance_children/y"),
            Record::parsed(Value::from(2i64)),
        )
        .expect("write failed");

    let mut children = store
        .read_children(&path!("conformance_children"))
        .expect("read_children failed")
        .expect("read_children must return Some at an existing prefix");
    children.sort();
    assert_eq!(
        children,
        vec!["x".to_string(), "y".to_string()],
        "read_children must enumerate direct children"
    );

    assert_eq!(
        store
            .read_children(&path!("conformance_children_missing"))
            .expect("read_children failed"),
        None,
        "read_children at a missing path must return None"
    );
}

/// Writing `Value::Null` deletes the node and its entire subtree, without
/// touching siblings whose names merely share a string prefix.
pub fn check_null_write_deletes_subtree<S: Store>(store: &mut S) {
    store
        .write(
            &path!("conformance_del/accounts/personal/key"),
            Record::parsed(Value::from("secret")),
        )
        .expect("write failed");
    store
        .write(
            &path!("conformance_del/accounts_other"),
            Record::parsed(Value::from("survivor")),
        )
        .expect("write failed");

    store
        .write(
            &path!("conformance_del/accounts"),
            Record::parsed(Value::Null),
        )
        .expect("null write failed");

    assert_eq!(
        read_value(store, &path!("conformance_del/accounts")),
        None,
        "null write must delete the node"
    );
    assert_eq!(
        read_value(store, &path!("conformance_del/accounts/personal/key")),
        None,
        "null write must delete the entire subtree"
    );
    assert_eq!(
        read_value(store, &path!("conformance_del/accounts_other")),
        Some(Value::from("survivor")),
        "null write must be component-wise: string-prefix siblings survive"
    );
}

/// Writing a `Value::Map` at a parent replaces the full state under that
/// path; stale descendants do not survive.
pub fn check_map_write_replaces_subtree<S: Store>(store: &mut S) {
    store
        .write(
            &path!("conformance_replace/cfg/stale"),
            Record::parsed(Value::from("old")),
        )
        .expect("write failed");

    let mut fresh = std::collections::BTreeMap::new();
    fresh.insert("fresh".to_string(), Value::from("new"));
    store
        .write(
            &path!("conformance_replace/cfg"),
            Record::parsed(Value::Map(fresh)),
        )
        .expect("map write failed");

    assert_eq!(
        read_value(store, &path!("conformance_replace/cfg/stale")),
        None,
        "map write at a parent must sweep stale descendants"
    );
    assert_eq!(
        read_value(store, &path!("conformance_replace/cfg/fresh")),
        Some(Value::from("new")),
        "map write must install the new state"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStore;

    #[test]
    fn memory_store_is_conformant() {
        check_conventions(&mut MemoryStore::new());
    }
}
