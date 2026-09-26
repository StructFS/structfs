use crate::{CallContext, Operation, Response, Service};
use std::sync::{Arc, Mutex};
use structfs_core_store::{
    DetachedFuture, DetachedShared, DetachedStore, Error, SharedReader, SharedWriter, Store,
};
use structfs_handles::Gate;

/// Every adapter here uses one poison policy: recover the guard with
/// `into_inner` rather than failing forever. A provider whose store panicked
/// mid-operation may be inconsistent, but permanently bricking the mount is
/// strictly worse than letting the next call observe the damage.
fn recover<T>(result: std::sync::LockResult<T>) -> T {
    result.unwrap_or_else(|e| e.into_inner())
}

/// A detached store's mutex is held only while constructing the future.
///
/// A thin `Service` face on [`DetachedShared`], which already owns the
/// shared-handle mechanics; this adds only the operation/response mapping.
pub struct DetachedProvider<T> {
    inner: DetachedShared<T>,
}
impl<T> DetachedProvider<T> {
    pub fn new(store: T) -> Self {
        Self {
            inner: DetachedShared::new(store),
        }
    }
}
impl<T: DetachedStore + 'static> Service for DetachedProvider<T> {
    fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
        match op {
            Operation::Read(p) => {
                let f = self.inner.read_shared(p);
                Box::pin(async move {
                    let _context = context;
                    f.await.map(Response::Read)
                })
            }
            Operation::Write(p, d) => {
                let f = self.inner.write_shared(p, d);
                Box::pin(async move {
                    let _context = context;
                    f.await.map(Response::Written)
                })
            }
        }
    }
}
/// Opt-in adapter for short, nonblocking synchronous work.
pub struct ImmediateStore<T> {
    inner: Mutex<T>,
}
impl<T> ImmediateStore<T> {
    pub fn new(store: T) -> Self {
        Self {
            inner: Mutex::new(store),
        }
    }
}
impl<T: Store + 'static> Service for ImmediateStore<T> {
    fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
        let result = (|| {
            context.ensure_active()?;
            let mut s = recover(self.inner.lock());
            match op {
                Operation::Read(p) => s.read(&p).map(Response::Read),
                Operation::Write(p, d) => s.write(&p, d).map(Response::Written),
            }
        })();
        Box::pin(async move {
            let _context = context;
            result
        })
    }
}
struct State {
    closed: bool,
    active: usize,
}
struct Work {
    state: Mutex<State>,
    gate: Gate,
}
struct Finished(Arc<Work>);
impl Drop for Finished {
    fn drop(&mut self) {
        let mut s = recover(self.0.state.lock());
        s.active -= 1;
        drop(s);
        self.0.gate.notify();
    }
}
/// Blocking I/O adapter. Cancellation abandons the result, not an already
/// running closure. The closure retains its lease; `close` stops admission and
/// `join` waits for work already inside the provider.
pub struct BlockingStore<T> {
    inner: Arc<Mutex<T>>,
    work: Arc<Work>,
}
impl<T> BlockingStore<T> {
    pub fn new(store: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
            work: Arc::new(Work {
                state: Mutex::new(State {
                    closed: false,
                    active: 0,
                }),
                gate: Gate::new(),
            }),
        }
    }
    /// Stop admitting calls. Non-blocking and idempotent; work already inside
    /// the provider keeps running until it finishes.
    pub fn close(&self) {
        recover(self.work.state.lock()).closed = true;
        self.work.gate.notify();
    }
    /// [`BlockingStore::close`], then wait at most `timeout` for in-flight
    /// work to finish. Returns whether it did; work still running keeps its
    /// admission lease either way.
    pub async fn join(&self, timeout: std::time::Duration) -> bool {
        self.close();
        tokio::time::timeout(
            timeout,
            self.work
                .gate
                .wait_until(|| (recover(self.work.state.lock()).active == 0).then_some(())),
        )
        .await
        .is_ok()
    }
    pub fn active(&self) -> usize {
        recover(self.work.state.lock()).active
    }
}
impl<T: Store + 'static> Service for BlockingStore<T> {
    fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
        let inner = self.inner.clone();
        let work = self.work.clone();
        Box::pin(async move {
            let done = {
                let mut state = recover(work.state.lock());
                if state.closed {
                    return Err(Error::cancelled("provider closed"));
                }
                context.ensure_active()?;
                state.active += 1;
                Finished(work.clone())
            };
            tokio::task::spawn_blocking(move || {
                let _done = done;
                let context = context;
                let _lease = context.lease();
                context.ensure_active()?;
                let mut store = recover(inner.lock());
                context.ensure_active()?;
                match op {
                    Operation::Read(p) => store.read(&p).map(Response::Read),
                    Operation::Write(p, d) => store.write(&p, d).map(Response::Written),
                }
            })
            .await
            .map_err(|_| Error::store("service", "blocking", "provider task failed"))?
        })
    }
}
