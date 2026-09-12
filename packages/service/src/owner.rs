//! Explicit ownership and a retained, bounded cleanup supervisor.
use crate::{next_id, CancelToken, Error};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use structfs_handles::Gate;

type Cleanup = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send>> + Send>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResourceId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Task,
    Provider,
    Registration,
    Retained,
    Child,
}
#[derive(Clone, Copy, Debug)]
pub struct OwnerLimits {
    pub resources: usize,
    pub retained_bytes: usize,
}
impl Default for OwnerLimits {
    fn default() -> Self {
        Self {
            resources: 1024,
            retained_bytes: 16 * 1024 * 1024,
        }
    }
}
#[derive(Clone, Debug)]
pub struct RemainingResource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub bytes: usize,
    pub failed: bool,
}
#[derive(Clone, Debug)]
pub struct CloseReport {
    pub owner_id: u64,
    pub closed: bool,
    pub remaining: Vec<RemainingResource>,
    /// Includes panics and cleanup errors; failures never imply successful release.
    pub failures: u64,
}
impl CloseReport {
    pub fn is_quiescent(&self) -> bool {
        self.closed && self.remaining.is_empty()
    }
}
struct Entry {
    kind: ResourceKind,
    bytes: usize,
    cleanup: Option<Cleanup>,
    cancel: CancelToken,
    failed: bool,
}
struct State {
    closed: bool,
    entries: BTreeMap<ResourceId, Entry>,
    bytes: usize,
    failures: u64,
}
struct Inner {
    id: u64,
    limits: OwnerLimits,
    state: Mutex<State>,
    cancel: CancelToken,
    gate: Gate,
    ancestors: Vec<CancelToken>,
    runtime: tokio::runtime::Handle,
}
impl Inner {
    fn report(&self) -> CloseReport {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        CloseReport {
            owner_id: self.id,
            closed: s.closed,
            failures: s.failures,
            remaining: s
                .entries
                .iter()
                .map(|(id, e)| RemainingResource {
                    id: *id,
                    kind: e.kind,
                    bytes: e.bytes,
                    failed: e.failed,
                })
                .collect(),
        }
    }
    fn finish(&self, id: ResourceId, failed: bool) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if failed {
            s.failures = s.failures.saturating_add(1);
            if let Some(e) = s.entries.get_mut(&id) {
                e.failed = true;
            }
            // Failed cleanup remains visible and charged. There is no automatic retry
            // of a potentially irreversible release operation.
        } else if let Some(e) = s.entries.remove(&id) {
            s.bytes -= e.bytes;
        }
        drop(s);
        self.gate.notify();
    }
    fn cleanup(self: &Arc<Self>, id: ResourceId) {
        let action = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get_mut(&id)
            .and_then(|e| {
                e.cancel.cancel();
                e.cleanup.take()
            });
        if let Some(action) = action {
            let this = self.clone();
            // Construct and poll cleanup inside the task so panics are observed too.
            let task = self.runtime.spawn(async move { action().await });
            self.runtime.spawn(async move {
                let failed = !matches!(task.await, Ok(Ok(())));
                this.finish(id, failed);
            });
        }
    }
    fn cancel(self: &Arc<Self>) {
        let ids = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.closed = true;
            s.entries.keys().copied().collect::<Vec<_>>()
        };
        self.cancel.cancel();
        for id in ids {
            self.cleanup(id);
        }
        self.gate.notify();
    }
    async fn join(&self) {
        self.gate
            .wait_until(|| {
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entries
                    .is_empty()
                    .then_some(())
            })
            .await
    }
}
/// Host authority retained through shutdown. Capacity bounds even leaked owners.
/// Keep the Tokio runtime alive until cleanup has completed or been reconciled.
pub struct CleanupSupervisor {
    runtime: tokio::runtime::Handle,
    owners: Mutex<BTreeMap<u64, Arc<Inner>>>,
    capacity: usize,
    closed: Mutex<bool>,
}
impl CleanupSupervisor {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        Ok(Self::with_handle(
            capacity,
            tokio::runtime::Handle::try_current().map_err(|_| {
                Error::store(
                    "service",
                    "owner",
                    "cleanup supervisor requires a Tokio runtime",
                )
            })?,
        ))
    }
    pub fn with_handle(capacity: usize, runtime: tokio::runtime::Handle) -> Self {
        Self {
            runtime,
            owners: Mutex::new(BTreeMap::new()),
            capacity,
            closed: Mutex::new(false),
        }
    }

    pub fn owner(&self, limits: OwnerLimits) -> Result<Owner, Error> {
        self.owner_with_ancestors(limits, Vec::new())
    }
    fn owner_with_ancestors(
        &self,
        limits: OwnerLimits,
        ancestors: Vec<CancelToken>,
    ) -> Result<Owner, Error> {
        let closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
        if *closed {
            return Err(Error::cancelled("supervisor closed"));
        }
        let mut owners = self.owners.lock().unwrap_or_else(|e| e.into_inner());
        owners.retain(|_, o| !o.report().is_quiescent());
        if owners.len() >= self.capacity {
            return Err(Error::overloaded("owner capacity exhausted"));
        }
        let inner = Arc::new(Inner {
            id: next_id(),
            limits,
            runtime: self.runtime.clone(),
            gate: Gate::new(),
            ancestors,
            cancel: CancelToken::new(),
            state: Mutex::new(State {
                closed: false,
                entries: BTreeMap::new(),
                bytes: 0,
                failures: 0,
            }),
        });
        owners.insert(inner.id, inner.clone());
        Ok(Owner {
            handle: OwnerHandle(inner),
            parent: None,
        })
    }
    pub fn reports(&self) -> Vec<CloseReport> {
        self.owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|o| o.report())
            .collect()
    }
    /// Reconcile a completed failure after the original owner has been dropped.
    pub fn acknowledge_failure(&self, owner_id: u64, resource: ResourceId) -> Result<(), Error> {
        let inner = self
            .owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&owner_id)
            .cloned()
            .ok_or_else(|| Error::conflict("unknown owner"))?;
        OwnerHandle(inner).acknowledge_failure(resource)
    }
    /// Cancel all owners and wait at most `timeout` in total. Dropping this future
    /// leaves cleanup running under the supervisor.
    pub async fn close(&self, timeout: Duration) -> Vec<CloseReport> {
        *self.closed.lock().unwrap_or_else(|e| e.into_inner()) = true;
        let owners: Vec<_> = self
            .owners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for o in &owners {
            o.cancel();
        }
        let _ = tokio::time::timeout(timeout, async {
            for o in &owners {
                o.join().await;
            }
        })
        .await;
        owners.iter().map(|o| o.report()).collect()
    }
}
impl Drop for CleanupSupervisor {
    fn drop(&mut self) {
        for o in self
            .owners
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            o.cancel();
        }
    }
}
/// The unique scope lifetime. Drop signals cancellation without blocking.
pub struct Owner {
    handle: OwnerHandle,
    parent: Option<Registration>,
}
impl Owner {
    pub fn handle(&self) -> OwnerHandle {
        self.handle.clone()
    }
    pub fn cancel(&self) {
        self.handle.0.cancel();
        if let Some(p) = &self.parent {
            p.release();
        }
    }
    pub async fn close(&self, timeout: Duration) -> CloseReport {
        self.cancel();
        let _ = tokio::time::timeout(timeout, self.handle.0.join()).await;
        self.handle.report()
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.cancel();
    }
}
/// Cloneable admission authority; clones do not extend the unique owner's lifetime.
#[derive(Clone)]
pub struct OwnerHandle(Arc<Inner>);
impl OwnerHandle {
    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn cancel(&self) {
        self.0.cancel();
    }
    pub async fn close(&self, timeout: Duration) -> CloseReport {
        self.cancel();
        let _ = tokio::time::timeout(timeout, self.0.join()).await;
        self.report()
    }
    pub fn cancellation(&self) -> CancelToken {
        self.0.cancel.clone()
    }
    pub fn report(&self) -> CloseReport {
        self.0.report()
    }
    pub fn ensure_open(&self) -> Result<(), Error> {
        if self.0.ancestors.iter().any(CancelToken::is_cancelled)
            || self
                .0
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed
        {
            Err(Error::cancelled("owner closed"))
        } else {
            Ok(())
        }
    }
    fn insert(
        &self,
        kind: ResourceKind,
        bytes: usize,
        cleanup: Option<Cleanup>,
    ) -> Result<ResourceId, Error> {
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.closed || self.0.ancestors.iter().any(CancelToken::is_cancelled) {
            return Err(Error::cancelled("owner closed"));
        }
        if s.entries.len() >= self.0.limits.resources
            || bytes > self.0.limits.retained_bytes.saturating_sub(s.bytes)
        {
            return Err(Error::overloaded("owner resource capacity exhausted"));
        }
        let id = ResourceId(next_id());
        s.bytes += bytes;
        s.entries.insert(
            id,
            Entry {
                kind,
                bytes,
                cleanup,
                cancel: CancelToken::new(),
                failed: false,
            },
        );
        Ok(id)
    }
    pub fn track(&self, kind: ResourceKind, bytes: usize) -> Result<Reservation, Error> {
        Ok(Reservation {
            inner: self.0.clone(),
            id: self.insert(kind, bytes, None)?,
        })
    }
    /// Reserve cleanup before starting an asynchronous handle open. Once started,
    /// the open is observed to completion even if delivery is abandoned. The
    /// opener returns the public value and its host-owned release callback.
    /// An opener returning Err must already have released any partial resources.
    pub async fn open<T, F, Fut, C, CFut>(
        &self,
        bytes: usize,
        open: F,
    ) -> Result<OwnedResource<T>, Error>
    where
        T: Send + 'static,
        F: FnOnce(CancelToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(T, C), Error>> + Send + 'static,
        C: FnOnce() -> CFut + Send + 'static,
        CFut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let (task_send, task_recv) = tokio::sync::oneshot::channel::<
            tokio::task::JoinHandle<Option<(Arc<Mutex<Option<T>>>, C)>>,
        >();
        let registration =
            self.register(ResourceKind::Registration, bytes, move || async move {
                // A dropped sender before dispatch means no handle was opened.
                let Ok(task) = task_recv.await else {
                    return Ok(());
                };
                let cleanup = task.await.map_err(|_| {
                    Error::store(
                        "service",
                        "open",
                        "handle opener failed before reporting its outcome",
                    )
                })?;
                if let Some((value, cleanup)) = cleanup {
                    cleanup().await?;
                    *value.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                Ok(())
            })?;
        let (send, recv) = tokio::sync::oneshot::channel();
        let cancel = self.cancellation();
        let task = self.0.runtime.spawn(async move {
            if cancel.is_cancelled() {
                let _ = send.send(Err(Error::cancelled("owner closed")));
                return None;
            }
            match open(cancel).await {
                Ok((value, cleanup)) => {
                    let value = Arc::new(Mutex::new(Some(value)));
                    let _ = send.send(Ok(value.clone()));
                    Some((value, cleanup))
                }
                Err(error) => {
                    let _ = send.send(Err(error));
                    None
                }
            }
        });
        // The registered receiver remains owned until this task has been joined.
        let _ = task_send.send(task);
        let cancellation = self.cancellation();
        let value = tokio::select! { biased;
            _ = cancellation.cancelled() => return Err(Error::cancelled("owner closed")),
            result = recv => result.map_err(|_| Error::store("service", "open", "handle opener failed"))??,
        };
        self.ensure_open()?;
        Ok(OwnedResource {
            value,
            registration,
        })
    }
    /// Register host cleanup BEFORE exposing a handle. Cleanup runs independently
    /// of ordinary request cancellation and must use restricted host authority.
    /// Capacity is reserved here, so cleanup does not require another admission.
    pub fn register<F, Fut>(
        &self,
        kind: ResourceKind,
        bytes: usize,
        cleanup: F,
    ) -> Result<Registration, Error>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let id = self.insert(kind, bytes, Some(Box::new(move || Box::pin(cleanup()))))?;
        let cancel = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .get(&id)
            .map(|e| e.cancel.clone())
            .unwrap_or_else(|| {
                let t = CancelToken::new();
                t.cancel();
                t
            });
        Ok(Registration {
            inner: self.0.clone(),
            id,
            cancel,
        })
    }
    pub fn child(
        &self,
        supervisor: &CleanupSupervisor,
        limits: OwnerLimits,
    ) -> Result<Owner, Error> {
        let mut ancestors = self.0.ancestors.clone();
        ancestors.push(self.cancellation());
        let mut child = supervisor.owner_with_ancestors(limits, ancestors)?;
        let inner = child.handle.0.clone();
        child.parent = Some(self.register(ResourceKind::Child, 0, move || async move {
            inner.cancel();
            inner.join().await;
            Ok(())
        })?);
        Ok(child)
    }
    /// Host acknowledgement after independently reconciling a failed resource.
    /// This does not run cleanup again or assert that an effect was rolled back.
    pub fn acknowledge_failure(&self, id: ResourceId) -> Result<(), Error> {
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.entries.get(&id).is_some_and(|e| e.failed) {
            return Err(Error::conflict("resource is not a completed failure"));
        }
        let e = s.entries.remove(&id).unwrap();
        s.bytes -= e.bytes;
        drop(s);
        self.0.gate.notify();
        Ok(())
    }
    /// Supervise cooperative work. Ignoring cancellation is permitted but remains
    /// visible and charged. Task errors and panics remain in the close report.
    pub fn spawn<F, Fut>(&self, work: F) -> Result<ResourceId, Error>
    where
        F: FnOnce(CancelToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Error>> + Send + 'static,
    {
        let id = self.insert(ResourceKind::Task, 0, None)?;
        let token = self.cancellation();
        let task = self.0.runtime.spawn(async move { work(token).await });
        let inner = self.0.clone();
        self.0.runtime.spawn(async move {
            inner.finish(id, !matches!(task.await, Ok(Ok(()))));
        });
        Ok(id)
    }
}
/// Last reservation drop releases accounting. Put it in a Lease to share it.
pub struct Reservation {
    inner: Arc<Inner>,
    id: ResourceId,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.inner.finish(self.id, false);
    }
}
/// Unique registration identity; dropping it initiates exactly-once cleanup.
pub struct Registration {
    inner: Arc<Inner>,
    id: ResourceId,
    cancel: CancelToken,
}
/// A delivered handle whose cleanup remains registered with its owner.
/// Access after release fails; copies of an underlying handle still require the
/// provider's ordinary capability checks.
pub struct OwnedResource<T> {
    value: Arc<Mutex<Option<T>>>,
    registration: Registration,
}
impl<T> OwnedResource<T> {
    /// Inspect the handle synchronously. The callback must return promptly;
    /// borrowed handles cannot escape into an asynchronous operation.
    pub fn with<R>(&self, inspect: impl FnOnce(&T) -> R) -> Result<R, Error> {
        if self.registration.cancellation().is_cancelled() {
            Err(Error::cancelled("resource released"))
        } else {
            let value = self.value.lock().unwrap_or_else(|e| e.into_inner());
            value
                .as_ref()
                .map(inspect)
                .ok_or_else(|| Error::cancelled("resource released"))
        }
    }
    pub fn release(&self) {
        self.registration.release();
    }
    pub async fn close(&self, timeout: Duration) -> CloseReport {
        self.registration.close(timeout).await
    }
}
impl Registration {
    pub fn id(&self) -> ResourceId {
        self.id
    }
    pub fn cancellation(&self) -> CancelToken {
        self.cancel.clone()
    }
    pub fn release(&self) {
        self.cancel.cancel();
        self.inner.cleanup(self.id);
    }
    /// Wait for this registration, then return the entire owner's resource report.
    pub async fn close(&self, timeout: Duration) -> CloseReport {
        self.release();
        let _ = tokio::time::timeout(
            timeout,
            self.inner.gate.wait_until(|| {
                (!self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entries
                    .contains_key(&self.id))
                .then_some(())
            }),
        )
        .await;
        self.inner.report()
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.release();
    }
}
