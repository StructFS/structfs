//! Composable store wrappers: capability restriction, layering, sharing,
//! and path confinement.

use std::sync::{Arc, Mutex};

use crate::{Error, Path, Reader, Record, Writer};

/// A read-only view of a store: reads pass through, writes are rejected
/// with a `PermissionDenied` error.
///
/// Useful for handing a store to code that should only observe it (display
/// layers, documentation consumers).
pub struct ReadOnly<S>(S);

impl<S> ReadOnly<S> {
    /// Wrap a store in a read-only view.
    pub fn new(inner: S) -> Self {
        Self(inner)
    }

    /// Unwrap, returning the inner store.
    pub fn into_inner(self) -> S {
        self.0
    }
}

impl<S: Reader> Reader for ReadOnly<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.0.read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        self.0.read_children(from)
    }
}

impl<S: Reader> Writer for ReadOnly<S> {
    fn write(&mut self, to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::permission_denied(format!(
            "store is read-only (write to {})",
            to
        )))
    }
}

/// A layered store: reads try the primary first, then fall back to the
/// secondary; writes always go to the primary.
///
/// This is layering (like an overlay filesystem), distinct from
/// `OverlayStore`, which *routes* by path prefix. Typical use: runtime
/// overrides cascading over immutable defaults.
pub struct Cascade<A, B> {
    primary: A,
    fallback: B,
}

impl<A, B> Cascade<A, B> {
    /// Layer `primary` over `fallback`.
    pub fn new(primary: A, fallback: B) -> Self {
        Self { primary, fallback }
    }

    /// Unwrap, returning `(primary, fallback)`.
    pub fn into_inner(self) -> (A, B) {
        (self.primary, self.fallback)
    }
}

impl<A: Reader, B: Reader> Reader for Cascade<A, B> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        match self.primary.read(from)? {
            Some(record) => Ok(Some(record)),
            None => self.fallback.read(from),
        }
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        match self.primary.read_children(from)? {
            Some(children) => Ok(Some(children)),
            None => self.fallback.read_children(from),
        }
    }
}

impl<A: Writer, B: Send + Sync> Writer for Cascade<A, B> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.primary.write(to, data)
    }
}

/// A cloneable, shareable handle to a store.
///
/// `Reader`/`Writer` take `&mut self`, so sharing a store between owners
/// requires a lock. `Shared` is that lock, packaged: it implements the
/// store traits over `Arc<Mutex<S>>` so callers don't hand-roll the
/// wrapper. Lock poisoning is recovered from (the store may be mid-update,
/// but path-level operations are individually atomic).
pub struct Shared<S> {
    inner: Arc<Mutex<S>>,
}

impl<S> Shared<S> {
    /// Wrap a store for shared access.
    pub fn new(inner: S) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Access the underlying store directly.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, S> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<S> Clone for Shared<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S: Reader> Reader for Shared<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.lock().read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        self.lock().read_children(from)
    }
}

impl<S: Writer> Writer for Shared<S> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.lock().write(to, data)
    }
}

/// A store confined to a subtree of another store.
///
/// Incoming paths are joined under `root` before reaching the inner store,
/// and result paths from writes have the root stripped (component-wise)
/// before being returned, so the root never leaks to callers. A write
/// result that escapes the root is an error rather than a leak.
pub struct Rooted<S> {
    root: Path,
    inner: S,
}

impl<S> Rooted<S> {
    /// Confine `inner` to the subtree at `root`.
    pub fn new(root: Path, inner: S) -> Self {
        Self { root, inner }
    }

    /// Unwrap, returning the inner store.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: Reader> Reader for Rooted<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.inner.read(&self.root.join(from))
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        self.inner.read_children(&self.root.join(from))
    }
}

impl<S: Writer> Writer for Rooted<S> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let result = self.inner.write(&self.root.join(to), data)?;
        result.strip_prefix(&self.root).ok_or_else(|| {
            Error::store(
                "rooted",
                "write",
                format!("inner store returned path outside root: {}", result),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, Value};
    use std::collections::HashMap;

    struct MapStore {
        data: HashMap<Path, Record>,
    }

    impl MapStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Reader for MapStore {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl Writer for MapStore {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[test]
    fn read_only_passes_reads_rejects_writes() {
        let mut inner = MapStore::new();
        inner
            .write(&path!("key"), Record::parsed(Value::from("v")))
            .unwrap();

        let mut ro = ReadOnly::new(inner);
        assert!(ro.read(&path!("key")).unwrap().is_some());

        let err = ro
            .write(&path!("key"), Record::parsed(Value::from("w")))
            .unwrap_err();
        assert!(matches!(err, Error::PermissionDenied { .. }));

        // Inner store unchanged
        let mut inner = ro.into_inner();
        assert_eq!(
            inner.read(&path!("key")).unwrap().unwrap().as_value(),
            Some(&Value::from("v"))
        );
    }

    #[test]
    fn cascade_layers_reads_and_writes_to_primary() {
        let mut fallback = MapStore::new();
        fallback
            .write(&path!("base"), Record::parsed(Value::from("default")))
            .unwrap();
        fallback
            .write(&path!("both"), Record::parsed(Value::from("under")))
            .unwrap();

        let mut primary = MapStore::new();
        primary
            .write(&path!("both"), Record::parsed(Value::from("over")))
            .unwrap();

        let mut cascade = Cascade::new(primary, fallback);

        // Fallback shows through where primary has nothing
        assert_eq!(
            cascade.read(&path!("base")).unwrap().unwrap().as_value(),
            Some(&Value::from("default"))
        );
        // Primary wins where both exist
        assert_eq!(
            cascade.read(&path!("both")).unwrap().unwrap().as_value(),
            Some(&Value::from("over"))
        );
        // Missing everywhere
        assert!(cascade.read(&path!("missing")).unwrap().is_none());

        // Writes land in primary only
        cascade
            .write(&path!("new"), Record::parsed(Value::from("x")))
            .unwrap();
        let (mut primary, mut fallback) = cascade.into_inner();
        assert!(primary.read(&path!("new")).unwrap().is_some());
        assert!(fallback.read(&path!("new")).unwrap().is_none());
    }

    #[test]
    fn shared_clones_access_same_store() {
        let shared = Shared::new(MapStore::new());
        let mut a = shared.clone();
        let mut b = shared;

        a.write(&path!("key"), Record::parsed(Value::from("v")))
            .unwrap();
        assert!(b.read(&path!("key")).unwrap().is_some());
    }

    #[test]
    fn shared_is_send_and_usable_across_threads() {
        let shared = Shared::new(MapStore::new());
        let mut clone = shared.clone();
        let handle = std::thread::spawn(move || {
            clone
                .write(&path!("from_thread"), Record::parsed(Value::from(1i64)))
                .unwrap();
        });
        handle.join().unwrap();
        assert!(shared.lock().read(&path!("from_thread")).unwrap().is_some());
    }

    #[test]
    fn rooted_confines_and_strips() {
        let mut rooted = Rooted::new(path!("export/v1"), MapStore::new());

        let result = rooted
            .write(&path!("users/alice"), Record::parsed(Value::from("a")))
            .unwrap();
        // Root is stripped from the result path
        assert_eq!(result, path!("users/alice"));

        // Data actually lives under the root
        let mut inner = rooted.into_inner();
        assert!(inner
            .read(&path!("export/v1/users/alice"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn rooted_reads_under_root() {
        let mut inner = MapStore::new();
        inner
            .write(&path!("jail/key"), Record::parsed(Value::from("v")))
            .unwrap();

        let mut rooted = Rooted::new(path!("jail"), inner);
        assert!(rooted.read(&path!("key")).unwrap().is_some());
        // Sibling paths outside the root are unreachable
        assert!(rooted.read(&path!("jail/key")).unwrap().is_none());
    }

    #[test]
    fn rooted_escaping_write_result_is_error() {
        /// Store whose write returns a path outside the requested subtree.
        struct EscapingStore;

        impl Reader for EscapingStore {
            fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
                Ok(None)
            }
        }

        impl Writer for EscapingStore {
            fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
                Ok(path!("elsewhere/entirely"))
            }
        }

        let mut rooted = Rooted::new(path!("jail"), EscapingStore);
        let err = rooted
            .write(&path!("key"), Record::parsed(Value::Null))
            .unwrap_err();
        assert!(err.to_string().contains("outside root"));
    }
}
