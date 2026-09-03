//! A first-party in-memory store implementing the StructFS conventions.

use crate::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// An in-memory tree store that implements the StructFS store conventions:
///
/// - **Reading a prefix returns its subtree** as a `Value::Map` of children.
/// - **Writing deep paths creates intermediate maps** as needed.
/// - **Writing `Value::Null` deletes** the node and its entire subtree
///   (component-wise: deleting `accounts` does not touch `accounts_other`).
/// - **Writing a `Value::Map` at a parent replaces the full state** under
///   that path — stale descendants do not survive.
///
/// This is the reference implementation certified by the
/// [`conformance`](crate::conformance) suite; use that suite to verify
/// other stores implement the same semantics.
///
/// # Example
///
/// ```rust
/// use structfs_core_store::{MemoryStore, Reader, Writer, Record, Value, path};
///
/// let mut store = MemoryStore::new();
/// store.write(&path!("users/alice/name"), Record::parsed(Value::from("Alice"))).unwrap();
///
/// // Reading the prefix returns the subtree
/// let users = store.read(&path!("users")).unwrap().unwrap();
/// assert!(users.as_value().unwrap().is_map());
///
/// // Null deletes the subtree
/// store.write(&path!("users"), Record::parsed(Value::Null)).unwrap();
/// assert!(store.read(&path!("users/alice/name")).unwrap().is_none());
/// ```
#[derive(Debug, Default)]
pub struct MemoryStore {
    root: Value,
}

impl MemoryStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self { root: Value::Null }
    }

    /// Create a store with initial contents.
    pub fn with_root(root: Value) -> Self {
        Self { root }
    }

    /// Borrow the root value.
    pub fn root(&self) -> &Value {
        &self.root
    }
}

impl Reader for MemoryStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() && self.root.is_null() {
            return Ok(None);
        }
        Ok(self.root.get(from).cloned().map(Record::parsed))
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        // Project children without cloning the subtree.
        if from.is_empty() && self.root.is_null() {
            return Ok(None);
        }
        Ok(self.root.get(from).map(|v| match v {
            Value::Map(map) => map.keys().cloned().collect(),
            Value::Array(arr) => (0..arr.len()).map(|i| i.to_string()).collect(),
            _ => Vec::new(),
        }))
    }
}

impl Writer for MemoryStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;

        // Null write deletes the node and its subtree.
        if value.is_null() {
            if to.is_empty() {
                self.root = Value::Null;
            } else {
                self.root.remove(to)?;
            }
            return Ok(to.clone());
        }

        if to.is_empty() {
            self.root = value;
            return Ok(to.clone());
        }

        // Writing below a Null root implicitly creates the root map;
        // Value::set then creates intermediate maps along the way.
        if self.root.is_null() {
            self.root = Value::map();
        }
        self.root.set(to, value)?;
        Ok(to.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

    #[test]
    fn deep_write_creates_intermediates() {
        let mut store = MemoryStore::new();
        store
            .write(&path!("a/b/c"), Record::parsed(Value::from(1i64)))
            .unwrap();
        assert!(store.read(&path!("a")).unwrap().is_some());
        assert!(store.read(&path!("a/b")).unwrap().is_some());
        assert_eq!(
            store.read(&path!("a/b/c")).unwrap().unwrap().as_value(),
            Some(&Value::Integer(1))
        );
    }

    #[test]
    fn null_deletes_subtree_component_wise() {
        let mut store = MemoryStore::new();
        store
            .write(
                &path!("accounts/personal"),
                Record::parsed(Value::from(1i64)),
            )
            .unwrap();
        store
            .write(&path!("accounts_other"), Record::parsed(Value::from(2i64)))
            .unwrap();

        store
            .write(&path!("accounts"), Record::parsed(Value::Null))
            .unwrap();

        assert!(store.read(&path!("accounts")).unwrap().is_none());
        assert!(store.read(&path!("accounts/personal")).unwrap().is_none());
        // The string-prefix sibling survives
        assert!(store.read(&path!("accounts_other")).unwrap().is_some());
    }

    #[test]
    fn map_write_replaces_subtree() {
        let mut store = MemoryStore::new();
        store
            .write(&path!("cfg/old"), Record::parsed(Value::from("stale")))
            .unwrap();

        let mut new_state = std::collections::BTreeMap::new();
        new_state.insert("fresh".to_string(), Value::from("new"));
        store
            .write(&path!("cfg"), Record::parsed(Value::Map(new_state)))
            .unwrap();

        assert!(store.read(&path!("cfg/old")).unwrap().is_none());
        assert!(store.read(&path!("cfg/fresh")).unwrap().is_some());
    }

    #[test]
    fn empty_store_reads_none_at_root() {
        let mut store = MemoryStore::new();
        assert!(store.read(&path!("")).unwrap().is_none());
        assert!(store.read_children(&path!("")).unwrap().is_none());
    }

    #[test]
    fn root_write_and_clear() {
        let mut store = MemoryStore::new();
        store
            .write(&path!(""), Record::parsed(Value::from("everything")))
            .unwrap();
        assert!(store.read(&path!("")).unwrap().is_some());

        store
            .write(&path!(""), Record::parsed(Value::Null))
            .unwrap();
        assert!(store.read(&path!("")).unwrap().is_none());
    }

    #[test]
    fn read_children_overridden() {
        let mut store = MemoryStore::new();
        store
            .write(&path!("m/a"), Record::parsed(Value::from(1i64)))
            .unwrap();
        store
            .write(&path!("m/b"), Record::parsed(Value::from(2i64)))
            .unwrap();
        assert_eq!(
            store.read_children(&path!("m")).unwrap(),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(store.read_children(&path!("m/a")).unwrap(), Some(vec![]));
        assert_eq!(store.read_children(&path!("missing")).unwrap(), None);
    }
}
