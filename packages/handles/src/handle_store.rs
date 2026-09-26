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
//!   cancel token fires (failing parked reads, but *not* protocol writes, so
//!   teardown writes can still land), `close` runs, and the entry becomes
//!   inaccessible immediately. Acknowledgement awaits `close_wait`.
//!   Pending/failed cleanup remains available to repeated releases. Unknown
//!   handles are a no-op. Abandoning a wait does not undo the release request:
//!   the entry is reclaimed by the next mint or release once
//!   [`HandleProtocol::close_complete`] reports the cleanup finished.
//! - Reads and writes below a released or unknown handle see `None` /
//!   `NotFound`.
//! - Reading the root (or `outstanding`) lists live handle paths.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};

use structfs_core_store::{
    DetachedFuture, DetachedReader, DetachedWriter, Error, NoCodec, Path, Record, Value,
};

use crate::gate::CancelToken;

/// Context handed to a protocol when a handle is opened.
///
/// Construct one with [`HandleCx::new`]; the struct is `#[non_exhaustive]`
/// so later releases can hand protocols more context without a breaking
/// change.
#[non_exhaustive]
pub struct HandleCx {
    /// The minted handle id.
    pub id: u64,
    /// Cancelled when the handle is released.
    ///
    /// Protocol *reads* that park should park cancellably on this token.
    /// Protocol *writes* must not: release is a request to tear the handle
    /// down, and a teardown write (a final "done" marker, an
    /// acknowledgement, a flush) still has to land after the token fires.
    /// Cancellation therefore fails parked reads and leaves writes open.
    pub cancel: CancelToken,
}

impl HandleCx {
    /// A context for handle `id` whose parked reads fail when `cancel` fires.
    pub fn new(id: u64, cancel: CancelToken) -> Self {
        Self { id, cancel }
    }
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

    /// Wait for cleanup requested by `close`. Must be repeatable and safe to
    /// abandon: unfinished cleanup belongs to a supervisor, not this future.
    /// The default is suitable only when `close` completes cleanup synchronously.
    ///
    /// **A protocol that overrides this must also override
    /// [`HandleProtocol::close_complete`]**, otherwise the store reaps the
    /// handle's table entry as soon as `close` returns — before the cleanup
    /// this future is waiting on has actually finished.
    fn close_wait(&self, _handle: Arc<Self::Handle>) -> DetachedFuture<()> {
        Box::pin(async { Ok(()) })
    }

    /// Whether requested cleanup completed successfully, so the released
    /// handle's table entry can be reaped.
    ///
    /// The default matches the default [`HandleProtocol::close_wait`]: `close`
    /// finishes cleanup synchronously, so the entry is reclaimable the moment
    /// `close` has run. Protocols with asynchronous cleanup override both, and
    /// keep returning `false` until cleanup succeeds — a failure stays
    /// uncollected until the cleanup owner acknowledges it.
    ///
    /// Called without the handle table locked, so an implementation may take
    /// its own locks.
    fn close_complete(&self, _handle: &Self::Handle) -> bool {
        true
    }

    /// Optional documentation served at `docs`.
    fn docs(&self) -> Option<Value> {
        None
    }
}

struct Entry<H> {
    handle: Arc<H>,
    cancel: CancelToken,
    close: Arc<Once>,
}

struct Inner<P: HandleProtocol> {
    protocol: P,
    next_id: AtomicU64,
    entries: Mutex<BTreeMap<u64, Entry<P::Handle>>>,
}

impl<P: HandleProtocol> Drop for Inner<P> {
    /// Dropping the last clone of a handle store releases every live
    /// handle: parked reads cancel and the protocol's `close` runs. A
    /// handle must never outlive its store — the RAII rule that keeps
    /// abandoned owners (a dropped response future, a dead spawner
    /// block) from leaking their handles forever.
    fn drop(&mut self) {
        let entries = std::mem::take(self.entries.get_mut().unwrap_or_else(|e| e.into_inner()));
        for (_, entry) in entries {
            entry.cancel.cancel();
            entry.close.call_once(|| self.protocol.close(entry.handle));
        }
    }
}

/// Generic handle store over a [`HandleProtocol`].
///
/// Cloneable; clones share the handle table. Implements the detached async
/// store traits.
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
                protocol,
                next_id: AtomicU64::new(0),
                entries: Mutex::new(BTreeMap::new()),
            }),
        }
    }

    fn lock_entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Entry<P::Handle>>> {
        self.inner.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reclaim cancelled entries whose cleanup has finished.
    ///
    /// Two kinds of entry end up cancelled without anyone removing them:
    /// a release whose future was dropped before its first poll (the removal
    /// lives in that future), and a handle whose token was cancelled by
    /// someone other than the store — a protocol can hand the store's token to
    /// an owner, as `structfs_service::SupervisedProtocol` does, and the owner
    /// closing cancels it. The second kind never had `close` run, so the sweep
    /// runs it first, exactly once, as a release would have.
    ///
    /// Every mint and every release sweeps, which bounds the table by the live
    /// handles plus those whose cleanup is still running (or failed and is
    /// awaiting acknowledgement), without an O(n) scan on the read path.
    ///
    /// `close` and `close_complete` are protocol code — for a supervised
    /// protocol they take the owner's mutex — so neither is ever called while
    /// the table is locked.
    fn sweep(&self) {
        let candidates: Vec<(u64, Arc<P::Handle>, Arc<Once>)> = {
            let entries = self.lock_entries();
            entries
                .iter()
                .filter(|(_, e)| e.cancel.is_cancelled())
                .map(|(id, e)| (*id, e.handle.clone(), e.close.clone()))
                .collect()
        };
        if candidates.is_empty() {
            return;
        }
        let reclaimable: Vec<u64> = candidates
            .into_iter()
            .filter(|(_, handle, close)| {
                close.call_once(|| self.inner.protocol.close(handle.clone()));
                self.inner.protocol.close_complete(handle)
            })
            .map(|(id, _, _)| id)
            .collect();
        if reclaimable.is_empty() {
            return;
        }
        let mut entries = self.lock_entries();
        for id in reclaimable {
            // Re-check under the lock: another thread may have raced a
            // release between the two critical sections.
            if entries
                .get(&id)
                .is_some_and(|e| e.cancel.is_cancelled() && e.close.is_completed())
            {
                entries.remove(&id);
            }
        }
    }

    /// The handle path for an id: `outstanding/{id}`.
    pub fn handle_path(id: u64) -> Path {
        Path::from_components(vec![OUTSTANDING.to_string(), id.to_string()])
    }

    /// Number of live handles.
    pub fn live_handles(&self) -> usize {
        self.lock_entries()
            .values()
            .filter(|e| !e.cancel.is_cancelled())
            .count()
    }

    /// The ids of all live handles, ascending.
    ///
    /// For meta/introspection lenses layered over a handle store.
    pub fn handle_ids(&self) -> Vec<u64> {
        self.lock_entries()
            .iter()
            .filter(|(_, e)| !e.cancel.is_cancelled())
            .map(|(id, _)| *id)
            .collect()
    }

    /// The protocol state of a live handle, if present.
    ///
    /// For meta/introspection lenses; regular operations should go
    /// through the store interface.
    pub fn get_handle(&self, id: u64) -> Option<Arc<P::Handle>> {
        self.get(id)
    }

    fn mint(&self, request: Value) -> Result<Path, Error> {
        self.sweep();
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let cancel = CancelToken::new();
        let handle = self
            .inner
            .protocol
            .open(HandleCx::new(id, cancel.clone()), request)?;
        self.lock_entries().insert(
            id,
            Entry {
                handle: Arc::new(handle),
                cancel,
                close: Arc::new(Once::new()),
            },
        );
        Ok(Self::handle_path(id))
    }

    fn release(&self, id: u64) -> DetachedFuture<()> {
        // Reclaim anything an abandoned release future left behind before
        // taking this one on.
        self.sweep();
        // Retain a tombstone until successful cleanup so repeated/concurrent
        // releases wait for the same producer rather than acknowledge early.
        let entry = self.lock_entries().get(&id).map(|entry| {
            entry.cancel.cancel();
            (entry.handle.clone(), entry.close.clone())
        });
        let Some((handle, close)) = entry else {
            return Box::pin(async { Ok(()) });
        };
        close.call_once(|| self.inner.protocol.close(handle.clone()));
        let wait = self.inner.protocol.close_wait(handle);
        let inner = Arc::downgrade(&self.inner);
        Box::pin(async move {
            wait.await?;
            if let Some(inner) = inner.upgrade() {
                inner
                    .entries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
            }
            Ok(())
        })
    }

    fn get(&self, id: u64) -> Option<Arc<P::Handle>> {
        self.lock_entries()
            .get(&id)
            .filter(|e| !e.cancel.is_cancelled())
            .map(|e| e.handle.clone())
    }

    fn listing(&self) -> Value {
        let items: Vec<Value> = self
            .lock_entries()
            .iter()
            .filter(|(_, entry)| !entry.cancel.is_cancelled())
            .map(|(id, _)| Value::String(Self::handle_path(*id).to_string()))
            .collect();
        let mut map = BTreeMap::new();
        map.insert("items".to_string(), Value::Array(items));
        Value::Map(map)
    }

    /// Parse `outstanding/{id}[/sub...]`; `None` if the path has another shape.
    fn parse_handle(path: &Path) -> Option<(u64, Path)> {
        if path.len() < 2 || &path[0] != OUTSTANDING {
            return None;
        }
        let id: u64 = path[1].parse().ok()?;
        Some((id, path.slice(2, path.len())))
    }
}

impl<P: HandleProtocol> DetachedReader for HandleStore<P> {
    fn read_detached(&mut self, from: &Path) -> DetachedFuture<Option<Record>> {
        // Root and bare `outstanding` list live handles.
        if from.is_empty() || (from.len() == 1 && &from[0] == OUTSTANDING) {
            let listing = self.listing();
            return Box::pin(async move { Ok(Some(Record::parsed(listing))) });
        }
        if from.len() == 1 && &from[0] == "docs" {
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
            let result = data.into_value(&NoCodec).and_then(|value| self.mint(value));
            return Box::pin(async move { result });
        }

        let Some((id, sub)) = Self::parse_handle(to) else {
            let path = to.clone();
            return Box::pin(async move { Err(Error::not_found(path)) });
        };

        if sub.is_empty() {
            // Direct handle write: Null releases, anything else conflicts.
            let result = match data.into_value(&NoCodec) {
                Err(e) => Err(e),
                Ok(value) if value.is_null() => {
                    let wait = self.release(id);
                    let path = to.clone();
                    return Box::pin(async move {
                        wait.await?;
                        Ok(path)
                    });
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
    use crate::gate::Gate;

    /// A minimal append-only log with an atomic "items plus terminal status"
    /// read, enough to exercise parked reads through the store.
    #[derive(Default)]
    struct Log {
        state: Mutex<(Vec<Value>, bool)>,
        gate: Gate,
    }

    impl Log {
        fn push(&self, value: Value) {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .0
                .push(value);
            self.gate.notify();
        }

        fn finish(&self) {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
            self.gate.notify();
        }

        fn is_done(&self) -> bool {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).1
        }

        /// Park until there are events past `seq` or the log is finished,
        /// then return them together with the terminal status.
        async fn read_from(&self, seq: u64, cancel: &CancelToken) -> Result<Value, Error> {
            self.gate
                .wait_until_cancellable(cancel, || {
                    let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    let (events, done) = &*state;
                    if (events.len() as u64) <= seq && !*done {
                        return None;
                    }
                    let start = (seq as usize).min(events.len());
                    let mut map = BTreeMap::new();
                    map.insert("items".to_string(), Value::Array(events[start..].to_vec()));
                    map.insert("next".to_string(), Value::Integer(events.len() as i64));
                    map.insert(
                        "status".to_string(),
                        Value::from(if *done { "done" } else { "open" }),
                    );
                    Some(Value::Map(map))
                })
                .await
                .map_err(|c| c.into_error("stream handle released"))
        }
    }

    /// Test protocol: each handle is an event log. Writes to `push` append,
    /// reads of `events/from/{n}` are atomic tail reads, reads of `status`
    /// return open/done, writes to `done` finish the log.
    struct StreamProtocol;

    struct StreamHandle {
        log: Log,
        cancel: CancelToken,
    }

    impl HandleProtocol for StreamProtocol {
        type Handle = StreamHandle;

        fn open(&self, cx: HandleCx, _request: Value) -> Result<Self::Handle, Error> {
            Ok(StreamHandle {
                log: Log::default(),
                cancel: cx.cancel,
            })
        }

        fn read(&self, handle: Arc<Self::Handle>, sub: Path) -> DetachedFuture<Option<Record>> {
            Box::pin(async move {
                if sub.len() == 3 && &sub[0] == "events" && &sub[1] == "from" {
                    let seq: u64 = sub[2]
                        .parse()
                        .map_err(|_| Error::invalid_argument("bad cursor"))?;
                    let page = handle.log.read_from(seq, &handle.cancel).await?;
                    return Ok(Some(Record::parsed(page)));
                }
                if sub.len() == 1 && &sub[0] == "status" {
                    let status = if handle.log.is_done() { "done" } else { "open" };
                    return Ok(Some(Record::parsed(Value::from(status))));
                }
                Ok(None)
            })
        }

        fn write(
            &self,
            handle: Arc<Self::Handle>,
            sub: Path,
            data: Record,
        ) -> DetachedFuture<Path> {
            Box::pin(async move {
                if sub.len() == 1 && &sub[0] == "push" {
                    let value = data.into_value(&NoCodec)?;
                    handle.log.push(value);
                    return Ok(sub);
                }
                if sub.len() == 1 && &sub[0] == "done" {
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
            .write_detached(
                &path.join(&Path::parse("push").unwrap()),
                parsed(Value::from(1i64)),
            )
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
        s.write_detached(
            &handle.join(&Path::parse("push").unwrap()),
            parsed(Value::from(1i64)),
        )
        .await
        .unwrap();
        s.write_detached(
            &handle.join(&Path::parse("done").unwrap()),
            parsed(Value::Null),
        )
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
        s.write_detached(&handle, parsed(Value::Null))
            .await
            .unwrap();
        let err = parked.await.unwrap().unwrap_err();
        assert!(err.is_cancelled());

        // Post-release reads see absence.
        assert!(s.read_detached(&handle).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dropping_the_store_releases_live_handles() {
        let s = store();
        let handle_path = {
            let mut s = s.clone();
            s.write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
                .await
                .unwrap()
        };

        // Detached futures don't hold the store: create the read future,
        // drop every store clone, and the parked read must cancel.
        let tail = handle_path.join(&Path::parse("events/from/0").unwrap());
        let fut = {
            let mut reader = s.clone();
            reader.read_detached(&tail)
        };
        let parked = tokio::spawn(fut);
        tokio::task::yield_now().await;
        drop(s);

        let err = tokio::time::timeout(std::time::Duration::from_secs(5), parked)
            .await
            .expect("parked read never resolved after store drop")
            .unwrap()
            .unwrap_err();
        assert!(err.is_cancelled());
    }

    #[tokio::test]
    async fn abandoned_release_future_does_not_leak_a_tombstone() {
        // A protocol that overrides neither `close_wait` nor `close_complete`
        // finishes its cleanup inside `close`. Dropping the Null-write future
        // before its first poll therefore skips the removal that future owns;
        // the next operation must reclaim the entry rather than keep it
        // forever.
        let mut s = store();
        let handle = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        assert_eq!(s.lock_entries().len(), 1);

        drop(s.write_detached(&handle, parsed(Value::Null)));
        assert_eq!(s.live_handles(), 0);
        assert_eq!(s.lock_entries().len(), 1, "tombstone retained until swept");

        let next = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r2")))
            .await
            .unwrap();
        let entries = s.lock_entries();
        assert_eq!(
            entries.len(),
            1,
            "released handle must not survive as a tombstone"
        );
        assert!(entries.contains_key(&1));
        drop(entries);
        assert_eq!(next.to_string(), "outstanding/1");
    }

    #[tokio::test]
    async fn externally_cancelled_handles_are_closed_and_reaped() {
        // A protocol may share the store's token with an owner that cancels
        // it on its own schedule. No Null write ever arrives for such a
        // handle, so the sweep must run `close` itself and reclaim the entry.
        let mut s = store();
        let path = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        let handle = s.get_handle(0).unwrap();
        handle.cancel.cancel();
        assert_eq!(s.live_handles(), 0);
        assert!(s.read_detached(&path).await.unwrap().is_none());
        assert!(!handle.log.is_done(), "close has not run yet");

        s.write_detached(&Path::parse("").unwrap(), parsed(Value::from("r2")))
            .await
            .unwrap();
        assert!(handle.log.is_done(), "the sweep ran the protocol's close");
        let entries = s.lock_entries();
        assert_eq!(entries.len(), 1);
        assert!(!entries.contains_key(&0));
    }

    #[tokio::test]
    async fn release_is_idempotent() {
        let mut s = store();
        let handle = s
            .write_detached(&Path::parse("").unwrap(), parsed(Value::from("r")))
            .await
            .unwrap();
        s.write_detached(&handle, parsed(Value::Null))
            .await
            .unwrap();
        // Second release of the same handle is a no-op, not an error.
        s.write_detached(&handle, parsed(Value::Null))
            .await
            .unwrap();
        // Releasing a handle that never existed is also fine.
        s.write_detached(
            &Path::parse("outstanding/999").unwrap(),
            parsed(Value::Null),
        )
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
