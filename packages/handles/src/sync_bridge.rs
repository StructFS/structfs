//! Sync facade over detached async stores.

use structfs_core_store::{DetachedStore, Error, Path, Reader, Record, Writer};

/// Adapts a detached async store to the synchronous `Reader`/`Writer`
/// traits by blocking on a tokio runtime handle.
///
/// # Contract
///
/// Operations must be called from a thread that is NOT a tokio runtime
/// worker (`Handle::block_on` panics inside a runtime context). The
/// intended callers are dedicated blocking threads — `spawn_blocking`
/// closures, or plain OS threads — which is exactly where synchronous
/// block code runs.
pub struct SyncBridge<S> {
    inner: S,
    runtime: tokio::runtime::Handle,
}

impl<S: DetachedStore> SyncBridge<S> {
    /// Bridge a detached store onto a runtime handle.
    pub fn new(inner: S, runtime: tokio::runtime::Handle) -> Self {
        Self { inner, runtime }
    }

    /// Unwrap, returning the inner store.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: DetachedStore + Sync> Reader for SyncBridge<S> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let fut = self.inner.read_detached(from);
        self.runtime.block_on(fut)
    }
}

impl<S: DetachedStore + Sync> Writer for SyncBridge<S> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let fut = self.inner.write_detached(to, data);
        self.runtime.block_on(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, MemoryStore, Shared, Value};

    #[test]
    fn bridge_runs_on_plain_thread() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let store = Shared::new(MemoryStore::new());
        let handle = runtime.handle().clone();

        let worker = std::thread::spawn(move || {
            let mut bridge = SyncBridge::new(store, handle);
            bridge
                .write(&path!("key"), Record::parsed(Value::from("v")))
                .unwrap();
            bridge.read(&path!("key")).unwrap().is_some()
        });
        assert!(worker.join().unwrap());
    }
}
