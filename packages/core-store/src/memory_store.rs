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
    root: Option<Value>,
}

impl MemoryStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self { root: None }
    }

    /// Create a store with initial contents.
    pub fn with_root(root: Value) -> Self {
        Self { root: Some(root) }
    }

    /// Construct a snapshot without replaying writes. Null and empty containers
    /// are preserved, including a present Null root. Duplicate paths and explicit
    /// ancestor/descendant overlaps are rejected independent of input order.
    /// Implicit parents are maps; numeric components do not infer arrays.
    pub fn from_entries(entries: impl IntoIterator<Item = (Path, Value)>) -> Result<Self, Error> {
        let mut entries: Vec<_> = entries.into_iter().collect();
        entries.sort_by(|(a, _), (b, _)| a.iter().cmp(b.iter()));
        for pair in entries.windows(2) {
            if pair[1].0.has_prefix(&pair[0].0) {
                return Err(Error::conflict(format!(
                    "snapshot paths overlap: '{}' and '{}'",
                    pair[0].0, pair[1].0
                )));
            }
        }
        let mut store = Self::new();
        for (path, value) in entries {
            // Path is already validated. Never use write here: Null is data in a snapshot.
            if path.is_empty() {
                store.root = Some(value);
            } else {
                store
                    .root
                    .get_or_insert_with(Value::map)
                    .set(&path, value)?;
            }
        }
        Ok(store)
    }

    /// Borrow the root value. None is empty; Some(Null) is an imported Null root.
    pub fn root(&self) -> Option<&Value> {
        self.root.as_ref()
    }
}

impl Reader for MemoryStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self
            .root
            .as_ref()
            .and_then(|root| root.get(from))
            .cloned()
            .map(Record::parsed))
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        Ok(self
            .root
            .as_ref()
            .and_then(|root| root.get(from))
            .map(|v| match v {
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
                self.root = None;
            } else if let Some(root) = &mut self.root {
                root.remove(to)?;
            }
            return Ok(to.clone());
        }

        if to.is_empty() {
            self.root = Some(value);
            return Ok(to.clone());
        }

        // Writing below a Null root implicitly creates the root map;
        // Value::set then creates intermediate maps along the way.
        let root = self.root.get_or_insert_with(Value::map);
        if root.is_null() {
            *root = Value::map();
        }
        root.set(to, value)?;
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
