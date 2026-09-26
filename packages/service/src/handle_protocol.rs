//! Join handle cleanup using the existing service owner and supervisor.
use crate::{OwnerHandle, Registration, ResourceKind};
use std::{future::Future, sync::Arc, time::Duration};
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
        // The cleanup task must not block an executor worker while synchronous
        // open is in progress. The channel retains late delivery through owner
        // cancellation; an open error/panic closes it without delivering state.
        let (deliver, opened) = tokio::sync::oneshot::channel::<Arc<P::Handle>>();
        let protocol = self.protocol.clone();
        let join = self.join.clone();
        // Register with the *store's* cancellation token rather than a fresh
        // one. The owner can close while `open` is still running; if the two
        // tokens were separate, the store would keep an entry whose token
        // never fires — listed as live, with reads failing `Cancelled`
        // instead of reporting the documented absence. One token means the
        // store and the owner always agree that the handle is gone.
        let registration = self.owner.register_with_cancellation(
            ResourceKind::Provider,
            0,
            cx.cancel.clone(),
            move || async move {
                if let Ok(value) = opened.await {
                    let requested = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        protocol.close(value.clone());
                    }));
                    // A shutdown-hook panic must not skip joining accepted
                    // work, and must not be swallowed if that join also fails.
                    let joined = join(value).await;
                    return match (requested, joined) {
                        (Err(_), Err(error)) => Err(Error::store(
                            "handle",
                            "close",
                            format!("shutdown hook panicked and cleanup failed: {error}"),
                        )),
                        (Err(_), Ok(())) => Err(Error::store(
                            "handle",
                            "close",
                            "shutdown hook panicked after cleanup joined",
                        )),
                        (Ok(()), joined) => joined,
                    };
                }
                Ok(())
            },
        )?;
        let value = Arc::new(
            self.protocol
                .open(HandleCx::new(cx.id, registration.cancellation()), request)?,
        );
        // The owner retains the receiver until opening resolves, even after
        // cancellation. Executor destruction is outside the hosting contract.
        let _ = deliver.send(value.clone());
        Ok(SupervisedHandle {
            value,
            registration,
        })
    }
    fn read(&self, handle: Arc<Self::Handle>, path: Path) -> DetachedFuture<Option<Record>> {
        // A released handle is absent, not an error — the same answer the
        // store gives once it observes the shared cancellation token.
        if handle.registration.cancellation().is_cancelled() {
            return Box::pin(async { Ok(None) });
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
        handle.registration.close();
    }
    fn close_wait(&self, handle: Arc<Self::Handle>) -> DetachedFuture<()> {
        let timeout = self.timeout;
        Box::pin(async move {
            let report = handle.registration.join(timeout).await;
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
