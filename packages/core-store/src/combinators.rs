//! Composable store wrappers: capability restriction, layering, sharing,
//! path confinement, and redaction.

use std::sync::{Arc, Mutex};

use crate::{Error, Path, PathPattern, Reader, Record, Value, Writer};

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
/// With the `async` feature, detached reads require a `DetachedShared` fallback:
/// `Cascade::new(primary, DetachedShared::new(fallback))`. This retains access
/// to the same fallback until a primary miss, without starting it speculatively.
/// Errors from the primary propagate without consulting the fallback.
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

/// A store that redacts sensitive paths on read.
///
/// Paths matching any pattern read back as the mask value instead of
/// their contents; existence is preserved (a masked path that exists
/// reads `Some(mask)`, a missing one reads `None`). Writes pass through
/// unchanged — masking is a read-side lens, not write protection (wrap
/// in [`ReadOnly`] for that).
///
/// Matching is **component-wise** via [`PathPattern`]: masking
/// `gate/api_key` does not mask `gate/api_key_other`, which a string
/// prefix check would.
pub struct Masked<S> {
    inner: S,
    patterns: Vec<PathPattern>,
    mask: Value,
}

impl<S> Masked<S> {
    /// Mask paths matching `patterns` with the default `"[masked]"`.
    pub fn new(inner: S, patterns: Vec<PathPattern>) -> Self {
        Self::with_mask(inner, patterns, Value::from("[masked]"))
    }

    /// Mask with a custom mask value.
    pub fn with_mask(inner: S, patterns: Vec<PathPattern>, mask: Value) -> Self {
        Self {
            inner,
            patterns,
            mask,
        }
    }

    /// Unwrap, returning the inner store.
    pub fn into_inner(self) -> S {
        self.inner
    }

    fn is_masked(&self, path: &Path) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(path))
    }
}

impl<S: Reader> Reader for Masked<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if self.is_masked(from) {
            // Preserve existence, redact content.
            return Ok(self
                .inner
                .read(from)?
                .map(|_| Record::parsed(self.mask.clone())));
        }
        self.inner.read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        // Child names are structure, not content; they stay visible even
        // under a masked prefix.
        self.inner.read_children(from)
    }
}

impl<S: Writer> Writer for Masked<S> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.inner.write(to, data)
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
    fn masked_redacts_component_wise() {
        let mut inner = MapStore::new();
        inner
            .write(
                &path!("gate/api_key"),
                Record::parsed(Value::from("s3cret")),
            )
            .unwrap();
        inner
            .write(
                &path!("gate/api_key_other"),
                Record::parsed(Value::from("visible")),
            )
            .unwrap();
        inner
            .write(&path!("gate/model"), Record::parsed(Value::from("gpt-oss")))
            .unwrap();

        let mut masked = Masked::new(inner, vec![PathPattern::prefix(path!("gate/api_key"))]);

        // The secret reads as the mask; existence is preserved.
        assert_eq!(
            masked
                .read(&path!("gate/api_key"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("[masked]"))
        );
        // The byte-prefix bug: a component-wise sibling stays visible.
        assert_eq!(
            masked
                .read(&path!("gate/api_key_other"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("visible"))
        );
        // Unmasked paths pass through.
        assert_eq!(
            masked
                .read(&path!("gate/model"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("gpt-oss"))
        );
        // Missing masked paths stay absent — no fabricated existence.
        assert!(masked.read(&path!("gate/api_key/sub")).unwrap().is_none());
    }

    #[test]
    fn masked_passes_writes_through() {
        let inner = MapStore::new();
        let mut masked = Masked::with_mask(
            inner,
            vec![PathPattern::exact(path!("secret"))],
            Value::Null,
        );
        masked
            .write(&path!("secret"), Record::parsed(Value::from("v")))
            .unwrap();
        // Read of the freshly written secret is masked (custom mask).
        assert_eq!(
            masked.read(&path!("secret")).unwrap().unwrap().as_value(),
            Some(&Value::Null)
        );
        // The inner store holds the real value.
        let mut inner = masked.into_inner();
        assert_eq!(
            inner.read(&path!("secret")).unwrap().unwrap().as_value(),
            Some(&Value::from("v"))
        );
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

#[cfg(feature = "async")]
mod detached {
    use super::*;
    use crate::{DetachedFuture, DetachedReader, DetachedWriter};

    /// Shared access to a detached store. The lock covers only operation
    /// construction and is released before polling the returned future.
    /// Unlike `Shared`, this delegates to detached operations, not sync ones.
    /// Acceptance and future-drop behavior are inherited from the provider.
    /// A panic during construction poisons the lock; all later operations fail
    /// without reentering the possibly inconsistent provider.
    pub struct DetachedShared<S>(Arc<Mutex<S>>);

    impl<S> DetachedShared<S> {
        pub fn new(inner: S) -> Self {
            Self(Arc::new(Mutex::new(inner)))
        }
    }
    impl<S> Clone for DetachedShared<S> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }
    impl<S: DetachedReader> crate::SharedReader for DetachedShared<S> {
        fn read(&self, path: Path) -> DetachedFuture<Option<Record>> {
            match self.0.lock() {
                Ok(mut store) => store.read_detached(&path),
                Err(_) => Box::pin(async {
                    Err(Error::store("shared", "read", "construction lock poisoned"))
                }),
            }
        }
    }
    impl<S: DetachedWriter> crate::SharedWriter for DetachedShared<S> {
        fn write(&self, path: Path, record: Record) -> DetachedFuture<Path> {
            match self.0.lock() {
                Ok(mut store) => store.write_detached(&path, record),
                Err(_) => Box::pin(async {
                    Err(Error::store(
                        "shared",
                        "write",
                        "construction lock poisoned",
                    ))
                }),
            }
        }
    }
    impl<S: DetachedReader> DetachedReader for DetachedShared<S> {
        fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
            crate::SharedReader::read(self, from.clone())
        }
    }
    impl<S: DetachedWriter> DetachedWriter for DetachedShared<S> {
        fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
            crate::SharedWriter::write(self, to.clone(), data)
        }
    }

    impl<S: DetachedReader> DetachedReader for ReadOnly<S> {
        fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
            self.0.read_detached(from)
        }
    }
    impl<S: Send> DetachedWriter for ReadOnly<S> {
        fn write_detached(&mut self, to: &Path, _: Record) -> DetachedFuture<Path> {
            let error = Error::permission_denied(format!("store is read-only (write to {to})"));
            Box::pin(async move { Err(error) })
        }
    }
    impl<S: DetachedReader> DetachedReader for Rooted<S> {
        fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
            self.inner.read_detached(&self.root.join(from))
        }
    }
    impl<S: DetachedWriter> DetachedWriter for Rooted<S> {
        fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
            let operation = self.inner.write_detached(&self.root.join(to), data);
            let root = self.root.clone();
            Box::pin(async move {
                let result = operation.await?;
                result.strip_prefix(&root).ok_or_else(|| {
                    Error::store(
                        "rooted",
                        "write",
                        format!("inner store returned path outside root: {result}"),
                    )
                })
            })
        }
    }
    impl<S: DetachedReader> DetachedReader for Masked<S> {
        fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
            let mask = self.is_masked(from).then(|| self.mask.clone());
            let operation = self.inner.read_detached(from);
            Box::pin(async move {
                let record = operation.await?;
                Ok(match mask {
                    Some(mask) => record.map(|_| Record::parsed(mask)),
                    None => record,
                })
            })
        }
    }
    impl<S: DetachedWriter> DetachedWriter for Masked<S> {
        fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
            self.inner.write_detached(to, data)
        }
    }
    // The shared fallback is intentional: after awaiting a primary miss we
    // must still access the SAME fallback, without borrowing the Cascade or
    // speculatively constructing an effectful fallback operation.
    impl<A: DetachedReader, B: DetachedReader + 'static> DetachedReader
        for Cascade<A, DetachedShared<B>>
    {
        fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
            let primary = self.primary.read_detached(from);
            let mut fallback = self.fallback.clone();
            let path = from.clone();
            Box::pin(async move {
                match primary.await? {
                    Some(record) => Ok(Some(record)),
                    None => fallback.read_detached(&path).await,
                }
            })
        }
    }
    impl<A: DetachedWriter, B: Send> DetachedWriter for Cascade<A, B> {
        fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
            self.primary.write_detached(to, data)
        }
    }
}
#[cfg(feature = "async")]
pub use detached::DetachedShared;
