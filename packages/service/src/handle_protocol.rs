//! Join handle cleanup using the existing service owner and supervisor.
use crate::{OwnerHandle, Registration, ResourceKind};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use structfs_core_store::{DetachedFuture, Error, Path, Record, Value};
use structfs_handles::{HandleCx, HandleProtocol};

/// A HandleStore protocol whose producer cleanup is registered before opening.
/// `join` must await all accepted producer work, publish its terminal state and
/// return failures. `protocol.close` requests shutdown; the owner retains the
/// join even when release futures or the final HandleStore are dropped.
/// Keep the owner/supervisor and its executor alive through drain.
pub struct SupervisedProtocol<P, F> {
    protocol: Arc<P>,
    owner: OwnerHandle,
    join: Arc<F>,
    timeout: Duration,
}
/// State and a cleanup registration share the lifetime of a handle.
pub struct SupervisedHandle<H> {
    value: Arc<H>,
    registration: Registration,
}
impl<P, F> SupervisedProtocol<P, F> {
    pub fn new(protocol: P, owner: OwnerHandle, timeout: Duration, join: F) -> Self {
        Self {
            protocol: Arc::new(protocol),
            owner,
            join: Arc::new(join),
            timeout,
        }
    }
}
impl<P, F, Fut> HandleProtocol for SupervisedProtocol<P, F>
where
    P: HandleProtocol,
    F: Fn(Arc<P::Handle>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), Error>> + Send + 'static,
{
    type Handle = SupervisedHandle<P::Handle>;
    fn open(&self, cx: HandleCx, request: Value) -> Result<Self::Handle, Error> {
        let slot = Arc::new(Mutex::new(None::<Arc<P::Handle>>));
        // Hold the slot while opening: a simultaneous owner close must observe
        // the eventual handle, even if it starts before open returns.
        let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
        let cleanup = slot.clone();
        let protocol = self.protocol.clone();
        let join = self.join.clone();
        let registration = self
            .owner
            .register(ResourceKind::Provider, 0, move || async move {
                let value = cleanup.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(value) = value {
                    protocol.close(value.clone());
                    join(value).await?;
                }
                Ok(())
            })?;
        let value = Arc::new(self.protocol.open(
            HandleCx {
                id: cx.id,
                cancel: registration.cancellation(),
            },
            request,
        )?);
        *guard = Some(value.clone());
        drop(guard);
        Ok(SupervisedHandle {
            value,
            registration,
        })
    }
    fn read(&self, handle: Arc<Self::Handle>, path: Path) -> DetachedFuture<Option<Record>> {
        if handle.registration.cancellation().is_cancelled() {
            return Box::pin(async { Err(Error::cancelled("handle released")) });
        }
        self.protocol.read(handle.value.clone(), path)
    }
    fn write(&self, handle: Arc<Self::Handle>, path: Path, data: Record) -> DetachedFuture<Path> {
        if handle.registration.cancellation().is_cancelled() {
            return Box::pin(async { Err(Error::cancelled("handle released")) });
        }
        self.protocol.write(handle.value.clone(), path, data)
    }
    fn close(&self, handle: Arc<Self::Handle>) {
        handle.registration.release();
    }
    fn close_wait(&self, handle: Arc<Self::Handle>) -> DetachedFuture<()> {
        let timeout = self.timeout;
        Box::pin(async move {
            let report = handle.registration.close(timeout).await;
            match report
                .remaining
                .iter()
                .find(|r| r.id == handle.registration.id())
            {
                Some(r) if r.failed => Err(Error::store(
                    "handle",
                    "close",
                    "producer cleanup failed; inspect owner report",
                )),
                Some(_) => Err(Error::deadline_exceeded(
                    "handle cleanup remains supervised",
                )),
                None => Ok(()),
            }
        })
    }
    fn close_complete(&self, handle: &Self::Handle) -> bool {
        handle.registration.is_complete()
    }
    fn docs(&self) -> Option<Value> {
        self.protocol.docs()
    }
}
