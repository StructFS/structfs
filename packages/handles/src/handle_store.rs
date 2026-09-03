//! `HandleStore`: generic `outstanding/{id}` handle scaffolding.
//!
//! The deferred-operation pattern — write a request to a store's root, get
//! back an `outstanding/{id}` handle path, then read the handle for results
//! — recurs in every broker-shaped store. This module owns the mechanics
//! (id minting, handle routing, the no-overwrite rule, Null-write release,
//! cancellation, listing) so a store author only implements the protocol:
//! what a handle *is* and how its sub-paths respond.
//!
//! # Protocol rules (enforced here)
//!
//! - A write to the store root **mints** a handle and returns
//!   `outstanding/{id}`.
//! - A non-Null write directly to `outstanding/{id}` is a **conflict** —
//!   handles cannot be overwritten.
//! - A Null write to `outstanding/{id}` **releases** the handle: its
//!   cancel token fires (failing parked reads), `close` runs, and the
//!   entry is removed. Releasing an unknown handle is a no-op (idempotent).
//! - Reads and writes below a released or unknown handle see `None` /
//!   `NotFound`.
//! - Reading the root (or `outstanding`) lists live handle paths.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use structfs_core_store::{
    DetachedFuture, DetachedReader, DetachedWriter, Error, NoCodec, Path, Record, Value,
};

use crate::gate::CancelToken;

/// Context handed to a protocol when a handle is opened.
pub struct HandleCx {
    /// The minted handle id.
    pub id: u64,
    /// Cancelled when the handle is released. Protocol reads that park
    /// should park cancellably on this token; writes should not, so
    /// teardown writes can still land.
    pub cancel: CancelToken,
}

/// The store-specific half of a handle store.
///
/// Implementations define per-handle state and the meaning of sub-paths
/// under `outstanding/{id}`. All routing and lifecycle is handled by
/// [`HandleStore`].
pub trait HandleProtocol: Send + Sync + 'static {
    /// Per-handle state. Stored behind an `Arc` so detached futures can
    /// hold it without borrowing the store.
    type Handle: Send + Sync + 'static;

    /// Open a handle for a request written to the store root.
    ///
    /// Spawn any background work here; keep `cx.cancel` if parked reads
    /// need to fail on release.
    fn open(&self, cx: HandleCx, request: Value) -> Result<Self::Handle, Error>;

    /// Serve a read below the handle. `sub` is relative to the handle
    /// (empty for a read of `outstanding/{id}` itself).
    fn read(&self, handle: Arc<Self::Handle>, sub: Path) -> DetachedFuture<Option<Record>>;

    /// Serve a write below the handle. The returned path is relative to
    /// the handle; [`HandleStore`] prefixes `outstanding/{id}` so callers
    /// always see paths in their own namespace.
    fn write(&self, handle: Arc<Self::Handle>, sub: Path, data: Record) -> DetachedFuture<Path>;

    /// Called once when the handle is released (after cancellation).
    fn close(&self, handle: Arc<Self::Handle>) {
        let _ = handle;
    }

    /// Optional documentation served at `docs`.
    fn docs(&self) -> Option<Value> {
        None
    }
}

struct Entry<H> {
    handle: Arc<H>,
    cancel: CancelToken,
}

struct Inner<P: HandleProtocol> {
    protocol: Arc<P>,
    next_id: AtomicU64,
    entries: Mutex<BTreeMap<u64, Entry<P::Handle>>>,
}

/// Generic handle store over a [`HandleProtocol`].
///
/// Cloneable; clones share the handle table. Implements the detached async
/// store traits — use [`crate::SyncBridge`] for the sync traits.
pub struct HandleStore<P: HandleProtocol> {
    inner: Arc<Inner<P>>,
}

impl<P: HandleProtocol> Clone for HandleStore<P> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

const OUTSTANDING: &str = "outstanding";

impl<P: HandleProtocol> HandleStore<P> {
    /// Create a handle store over a protocol.
    pub fn new(protocol: P) -> Self {
        Self {
            inner: Arc::new(Inner {
                protocol: Arc::new(protocol),
                next_id: AtomicU64::new(0),
                entries: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Entry<P::Handle>>> {
        self.inner.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The handle path for an id: `outstanding/{id}`.
    pub fn handle_path(id: u64) -> Path {
        Path::from_components(vec![OUTSTANDING.to_string(), id.to_string()])
    }

    /// Number of live handles.
    pub fn live_handles(&self) -> usize {
        self.lock_entries().len()
    }

    fn mint(&self, request: Value) -> Result<Path, Error> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let cancel = CancelToken::new();
        let handle = self.inner.protocol.open(
            HandleCx {
                id,
                cancel: cancel.clone(),
            },
            request,
        )?;
        self.lock_entries().insert(
            id,
            Entry {
                handle: Arc::new(handle),
                cancel,
            },
        );
        Ok(Self::handle_path(id))
    }

    fn release(&self, id: u64) {
        let entry = self.lock_entries().remove(&id);
        if let Some(entry) = entry {
            // Cancel first so parked reads fail, then let the protocol
            // tear down. Writes are unaffected by cancellation.
            entry.cancel.cancel();
            self.inner.protocol.close(entry.handle);
        }
    }

    fn get(&self, id: u64) -> Option<Arc<P::Handle>> {
        self.lock_entries().get(&id).map(|e| e.handle.clone())
    }

    fn listing(&self) -> Value {
        let items: Vec<Value> = self
            .lock_entries()
            .keys()
            .map(|id| Value::String(Self::handle_path(*id).to_string()))
            .collect();
        let mut map = BTreeMap::new();
        map.insert("items".to_string(), Value::Array(items));
        Value::Map(map)
    }

    /// Parse `outstanding/{id}[/sub...]`; `None` if the path has another shape.
    fn parse_handle(path: &Path) -> Option<(u64, Path)> {
        if path.len() < 2 || path[0] != OUTSTANDING {
            return None;
        }
        let id: u64 = path[1].parse().ok()?;
        Some((id, path.slice(2, path.len())))
    }
}

impl<P: HandleProtocol> DetachedReader for HandleStore<P> {
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
        // Root and bare `outstanding` list live handles.
        if from.is_empty() || (from.len() == 1 && from[0] == OUTSTANDING) {
            let listing = self.listing();
            return Box::pin(async move { Ok(Some(Record::parsed(listing))) });
        }
        if from.len() == 1 && from[0] == "docs" {
            let docs = self.inner.protocol.docs();
            return Box::pin(async move { Ok(docs.map(Record::parsed)) });
        }
        let Some((id, sub)) = Self::parse_handle(from) else {
            return Box::pin(async move { Ok(None) });
        };
        let Some(handle) = self.get(id) else {
            // Unknown or released handle: absent, not an error.
            return Box::pin(async move { Ok(None) });
        };
        self.inner.protocol.read(handle, sub)
    }
}

impl<P: HandleProtocol> DetachedWriter for HandleStore<P> {
    fn write_detached(&mut self, to: &Path, data: Record) -> DetachedFuture<Path> {
        // Root write mints a handle.
        if to.is_empty() {
            let result = data
                .into_value(&NoCodec)
                .and_then(|value| self.mint(value));
            return Box::pin(async move { result });
        }

        let Some((id, sub)) = Self::parse_handle(to) else {
            let path = to.clone();
            return Box::pin(async move {
                Err(Error::store(
                    "handle_store",
                    "write",
                    format!("no such path: {}", path),
                ))
            });
        };

        if sub.is_empty() {
            // Direct handle write: Null releases, anything else conflicts.
            let result = match data.into_value(&NoCodec) {
                Err(e) => Err(e),
                Ok(value) if value.is_null() => {
                    self.release(id);
                    Ok(to.clone())
                }
                Ok(_) => Err(Error::conflict(format!(
                    "cannot overwrite outstanding handle {}; write Null to release it",
                    Self::handle_path(id)
                ))),
            };
            return Box::pin(async move { result });
        }

        let Some(handle) = self.get(id) else {
            let path = to.clone();
            return Box::pin(async move { Err(Error::not_found(path)) });
        };
        let fut = self.inner.protocol.write(handle, sub, data);
        // Protocol write results are handle-relative; express them in the
        // caller's namespace.
        Box::pin(async move {
            let rel = fut.await?;
            Ok(Self::handle_path(id).join(&rel))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tail::TailLog;

    /// Test protocol: each handle is an event log. Writes to `push` append,
    /// reads of `events/from/{n}` are atomic tail reads, reads of `status`
    /// return open/done, writes to `done` finish the log.
    struct StreamProtocol;

    struct StreamHandle {
        log: TailLog<Value>,
        cancel: CancelToken,
    }

    impl HandleProtocol for StreamProtocol {
        type Handle = StreamHandle;

        fn open(&self, cx: HandleCx, _request: Value) -> Result<Self::Handle, Error> {
            Ok(StreamHandle {
                log: TailLog::new(),
                cancel: cx.cancel,
            })
        }

        fn read(&self, handle: Arc<Self::Handle>, sub: Path) -> DetachedFuture<Option<Record>> {
            Box::pin(async move {
                if sub.len() == 3 && sub[0] == "events" && sub[1] == "from" {
                    let seq: u64 = sub[2]
                        .parse()
                        .map_err(|_| Error::store("stream", "read", "bad cursor"))?;
                    let page = handle
                        .log
                        .read_from_cancellable(seq, &handle.cancel)
                        .await
                        .map_err(|c| c.into_error("stream handle released"))?;
                    return Ok(Some(Record::parsed(page.into_value())));
                }
                if sub.len() == 1 && sub[0] == "status" {
                    let status = if handle.log.is_done() { "done" } else { "open" };
                    return Ok(Some(Record::parsed(Value::from(status))));
                }
                Ok(None)
            })
        }

        fn write(&self, handle: Arc<Self::Handle>, sub: Path, data: Record) -> DetachedFuture<Path> {
            Box::pin(async move {
                if sub.len() == 1 && sub[0] == "push" {
                    let value = data.into_value(&NoCodec)?;
                    handle.log.push(value);
                    return Ok(sub);
                }
                if sub.len() == 1 && sub[0] == "done" {
                    handle.log.finish();
                    return Ok(sub);
                }
                Err(Error::store("stream", "write", "unknown sub-path"))
            })
        }

        fn close(&self, handle: Arc<Self::Handle>) {
            handle.log.finish();
        }
    }

    fn store() -> HandleStore<StreamProtocol> {
        HandleStore::new(StreamProtocol)
    }

    fn parsed(v: Value) -> Record {
        Record::parsed(v)
    }

    #[tokio::test]
    async fn mint_returns_handle_path() {
        let mut s = store();
        let path = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("req")))
            .await
            .unwrap();
        assert_eq!(path.to_string(), "outstanding/0");

        let second = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("req")))
            .await
            .unwrap();
        assert_eq!(second.to_string(), "outstanding/1");
    }

    #[tokio::test]
    async fn overwrite_is_conflict() {
        let mut s = store();
        let path = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        let err = s
            .write_detached(&path, parsed(Value::from("clobber")))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict { .. }));
    }

    #[tokio::test]
    async fn write_result_is_in_caller_namespace() {
        let mut s = store();
        let path = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        let result = s
            .write_detached(&path.join(&Path::parse("push").unwrap()), parsed(Value::from(1i64)))
            .await
            .unwrap();
        assert_eq!(result.to_string(), format!("{}/push", path));
    }

    #[tokio::test]
    async fn tail_read_through_store() {
        let mut s = store();
        let handle = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        s.write_detached(&handle.join(&Path::parse("push").unwrap()), parsed(Value::from(1i64)))
            .await
            .unwrap();
        s.write_detached(&handle.join(&Path::parse("done").unwrap()), parsed(Value::Null))
            .await
            .unwrap();

        let record = s
            .read_detached(&handle.join(&Path::parse("events/from/0").unwrap()))
            .await
            .unwrap()
            .unwrap();
        let map = match record.as_value().unwrap() {
            Value::Map(m) => m.clone(),
            _ => panic!("expected envelope"),
        };
        assert_eq!(map.get("status"), Some(&Value::from("done")));
        assert!(matches!(map.get("items"), Some(Value::Array(a)) if a.len() == 1));
    }

    #[tokio::test]
    async fn release_cancels_parked_reads() {
        let mut s = store();
        let handle = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();

        // Park a tail read with no events.
        let mut reader = s.clone();
        let tail_path = handle.join(&Path::parse("events/from/0").unwrap());
        let parked = tokio::spawn(async move { reader.read_detached(&tail_path).await });
        tokio::task::yield_now().await;

        // Release the handle: the parked read must fail with Cancelled.
        s.write_detached(&handle, parsed(Value::Null)).await.unwrap();
        let err = parked.await.unwrap().unwrap_err();
        assert!(err.is_cancelled());

        // Post-release reads see absence.
        assert!(s.read_detached(&handle).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_is_idempotent() {
        let mut s = store();
        let handle = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        s.write_detached(&handle, parsed(Value::Null)).await.unwrap();
        // Second release of the same handle is a no-op, not an error.
        s.write_detached(&handle, parsed(Value::Null)).await.unwrap();
        // Releasing a handle that never existed is also fine.
        s.write_detached(&Path::parse("outstanding/999").unwrap(), parsed(Value::Null))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn listing_tracks_live_handles() {
        let mut s = store();
        let a = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("a")))
            .await
            .unwrap();
        let _b = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("b")))
            .await
            .unwrap();

        let listing = s
            .read_detached(&Path::parse("outstanding").unwrap())
            .await
            .unwrap()
            .unwrap();
        let items = match listing.as_value().unwrap() {
            Value::Map(m) => match m.get("items").unwrap() {
                Value::Array(a) => a.len(),
                _ => panic!(),
            },
            _ => panic!(),
        };
        assert_eq!(items, 2);

        s.write_detached(&a, parsed(Value::Null)).await.unwrap();
        assert_eq!(s.live_handles(), 1);
    }

    #[tokio::test]
    async fn unknown_paths_absent() {
        let mut s = store();
        assert!(s
            .read_detached(&Path::parse("outstanding/42").unwrap())
            .await
            .unwrap()
            .is_none());
        assert!(s
            .read_detached(&Path::parse("something/else").unwrap())
            .await
            .unwrap()
            .is_none());
        let err = s
            .write_detached(
                &Path::parse("outstanding/42/push").unwrap(),
                parsed(Value::from(1i64)),
            )
            .await
            .unwrap_err();
        assert!(err.is_not_found());
    }

    #[test]
    fn handle_path_component_is_valid() {
        let p = HandleStore::<StreamProtocol>::handle_path(7);
        assert_eq!(p.to_string(), "outstanding/7");
        let _ = structfs_core_store::PathComponent::try_new("outstanding").unwrap();
    }
}
