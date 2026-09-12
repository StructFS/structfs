use crate::{CallContext, Operation, Response, Service};
use std::sync::{Arc, Mutex};
use structfs_core_store::{DetachedFuture, DetachedStore, Error, Store};
use structfs_handles::Gate;
fn poisoned() -> Error {
    Error::store("service", "dispatch", "provider lock poisoned")
}
/// A detached store's mutex is held only while constructing the future.
pub struct DetachedProvider<T> {
    inner: Mutex<T>,
}
impl<T> DetachedProvider<T> {
    pub fn new(store: T) -> Self {
        Self {
            inner: Mutex::new(store),
        }
    }
}
impl<T: DetachedStore + 'static> Service for DetachedProvider<T> {
    fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
        let mut store = match self.inner.lock() {
            Ok(s) => s,
            Err(_) => return Box::pin(async { Err(poisoned()) }),
        };
        match op {
            Operation::Read(p) => {
                let f = store.read_detached(&p);
                Box::pin(async move {
                    let _context = context;
                    f.await.map(Response::Read)
                })
            }
            Operation::Write(p, d) => {
                let f = store.write_detached(&p, d);
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
            let mut s = self.inner.lock().map_err(|_| poisoned())?;
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
        let mut s = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        s.active -= 1;
        drop(s);
        self.0.gate.notify();
    }
}
/// Blocking I/O adapter. Cancellation abandons the result, not an already running
/// closure. The closure retains its lease; `close` stops admission and joins work.
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
    pub async fn close(&self) {
        self.work
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        self.work
            .gate
            .wait_until(|| {
                (self
                    .work
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .active
                    == 0)
                    .then_some(())
            })
            .await;
    }
    pub fn active(&self) -> usize {
        self.work
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
    }
}
impl<T: Store + 'static> Service for BlockingStore<T> {
    fn call(&self, context: CallContext, op: Operation) -> DetachedFuture<Response> {
        let inner = self.inner.clone();
        let work = self.work.clone();
        Box::pin(async move {
            let done = {
                let mut state = work.state.lock().map_err(|_| poisoned())?;
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
                let mut store = inner.lock().map_err(|_| poisoned())?;
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
