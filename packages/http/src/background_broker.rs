//! The background HTTP broker: write queues a request that starts at once.

use std::sync::Arc;
use std::time::Duration;

use structfs_core_store::{
    DetachedReader, DetachedWriter, Error, Path, Reader, Record, Value, Writer,
};
use structfs_handles::HandleStore;

use crate::broker_common::{
    broker_docs, outstanding_listing, read_meta, root_references, BrokerKind, HandleMeta,
    RequestId, DOCS_PATH, META_PATH, OUTSTANDING_PREFIX,
};
use crate::executor::{BlockingHttpExecutor, BlockingReqwestExecutor};
use crate::handle_broker::{Execution, HttpBrokerProtocol};

/// HTTP broker whose requests execute on background threads.
///
/// A **synchronous** facade over a [`HandleStore`] running
/// `HttpBrokerProtocol`: the `outstanding/{id}` scaffolding — id minting,
/// the no-overwrite rule, Null-write release with cancellation, listing —
/// comes from `structfs-handles`. Requests execute on background threads;
/// `response/wait` is a parked read (no sleep-polling) that deleting the
/// handle cancels.
///
/// The name says *background*, not *async*, because the store itself is
/// synchronous: it implements [`Reader`]/[`Writer`], not the async traits.
/// The async counterpart is [`crate::streaming::AsyncHttpExecutor`].
///
/// ## Path Structure
///
/// | Path | Operation | Result |
/// |------|-----------|--------|
/// | `write /` | Queue request | Returns `outstanding/{id}` |
/// | `read /outstanding` | List handles | Returns `{items: [references]}` |
/// | `read /outstanding/{id}` | Check status | Returns `RequestStatus` |
/// | `read /outstanding/{id}/request` | View queued request | Returns original request |
/// | `read /outstanding/{id}/response` | Get response (non-blocking) | Returns response, or absent if pending |
/// | `read /outstanding/{id}/response/wait` | Get response (blocking) | Blocks until response ready |
/// | `write /outstanding/{id} null` | Delete handle | Removes handle |
///
/// # Runtime constraints
///
/// This store **owns a small Tokio runtime** and drives the detached handle
/// store on it with `block_on`. It must therefore not be called from inside
/// another Tokio runtime — call it from ordinary or blocking threads, which
/// is where sync stores run. The default executor additionally builds a
/// blocking reqwest client, which panics if *constructed* inside a runtime.
///
/// # Examples
///
/// ```no_run
/// use structfs_core_store::{path, Reader, Record, Writer};
/// use structfs_http::{BackgroundHttpBrokerStore, HttpRequest};
/// use structfs_serde_store::to_value;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut broker = BackgroundHttpBrokerStore::with_default_timeout()?;
///
/// // Queuing starts the request immediately.
/// let request = HttpRequest::get("https://example.com");
/// let handle = broker.write(&path!(""), Record::parsed(to_value(&request)?))?;
///
/// // Poll the status, or park until the response is ready.
/// let status = broker.read(&handle)?;
/// let response = broker.read(&handle.join(&path!("response/wait")))?;
/// # Ok(())
/// # }
/// ```
pub struct BackgroundHttpBrokerStore<E: BlockingHttpExecutor + 'static = BlockingReqwestExecutor> {
    runtime: tokio::runtime::Runtime,
    store: HandleStore<HttpBrokerProtocol<E>>,
    timeout: Duration,
}

impl BackgroundHttpBrokerStore<BlockingReqwestExecutor> {
    /// Create a new background HTTP broker with the given request timeout.
    ///
    /// # Panics
    ///
    /// Panics if called from within a Tokio runtime; see the type docs.
    pub fn new(timeout: Duration) -> Result<Self, crate::Error> {
        Self::with_executor(BlockingReqwestExecutor::new(timeout)?, timeout)
    }

    /// Create with default timeout of 30 seconds.
    ///
    /// # Panics
    ///
    /// Panics if called from within a Tokio runtime; see the type docs.
    pub fn with_default_timeout() -> Result<Self, crate::Error> {
        Self::new(Duration::from_secs(30))
    }
}

impl<E: BlockingHttpExecutor + 'static> BackgroundHttpBrokerStore<E> {
    /// Create a background broker over a custom executor.
    ///
    /// `timeout` is recorded for [`BackgroundHttpBrokerStore::timeout`];
    /// enforcing it is the executor's job.
    pub fn with_executor(executor: E, timeout: Duration) -> Result<Self, crate::Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| crate::Error::Runtime {
                message: e.to_string(),
            })?;
        Ok(Self {
            runtime,
            store: HandleStore::new(HttpBrokerProtocol {
                execution: Execution::Threaded(Arc::new(executor)),
            }),
            timeout,
        })
    }

    /// The per-request timeout this broker was created with.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Drive a detached-store operation to completion on the owned runtime.
    fn block_on<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        self.runtime.block_on(fut)
    }

    fn handle_meta(&self, id: RequestId) -> Option<HandleMeta> {
        let handle = self.store.get_handle(id)?;
        let status = handle.status();
        Some(HandleMeta {
            status: if status.is_complete() {
                "complete"
            } else if status.is_failed() {
                "failed"
            } else {
                "pending"
            },
            method: format!("{:?}", handle.request().method),
            url: handle.request().path.clone(),
        })
    }

    /// Whether a path addresses a handle: `outstanding/{numeric id}/...`.
    fn is_handle_path(path: &Path) -> bool {
        path.len() >= 2 && &path[0] == OUTSTANDING_PREFIX && path[1].parse::<RequestId>().is_ok()
    }
}

impl<E: BlockingHttpExecutor + 'static> Reader for BackgroundHttpBrokerStore<E> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(root_references())));
        }

        if &from[0] == DOCS_PATH {
            return Ok(Some(Record::parsed(broker_docs(BrokerKind::Background))));
        }

        if &from[0] == META_PATH {
            let ids = self.store.handle_ids();
            return read_meta(BrokerKind::Background, from, &ids, |id| {
                self.handle_meta(id)
            });
        }

        if from.len() == 1 && &from[0] == OUTSTANDING_PREFIX {
            return Ok(Some(Record::parsed(outstanding_listing(
                &self.store.handle_ids(),
            ))));
        }

        // Handle paths delegate to the handle store. Released or unknown
        // handles read as absent (the handle-protocol rule), and
        // response/wait parks on a gate instead of sleep-polling.
        if Self::is_handle_path(from) {
            let mut store = self.store.clone();
            return self.block_on(store.read_detached(from));
        }

        Err(Error::invalid_argument(format!(
            "Invalid path '{}'. Expected: outstanding, outstanding/{{id}}, or outstanding/{{id}}/...",
            from
        )))
    }
}

impl<E: BlockingHttpExecutor + 'static> Writer for BackgroundHttpBrokerStore<E> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // A handle that does not exist cannot conflict: any non-Null write
        // at `outstanding/{id}` for an unknown id is NotFound, matching the
        // sync broker. `HandleStore` itself answers Conflict for every
        // non-Null direct handle write without looking the id up, so the
        // facade checks first. (Writes *below* an unknown handle are already
        // NotFound in `HandleStore`; Null stays an idempotent release.)
        if to.len() == 2 && Self::is_handle_path(to) {
            let is_null = matches!(data.as_value(), Some(Value::Null));
            let id: RequestId = to[1].parse().unwrap_or(RequestId::MAX);
            if !is_null && self.store.get_handle(id).is_none() {
                return Err(Error::not_found(to.clone()));
            }
        }

        // Queue (root write), delete (Null to outstanding/{id}), and the
        // no-overwrite conflict rule are all handle-store scaffolding.
        if to.is_empty() || Self::is_handle_path(to) {
            let mut store = self.store.clone();
            return self.block_on(store.write_detached(to, data));
        }

        Err(Error::invalid_argument(format!(
            "Invalid write path '{}'. Write to root to queue a request, or write null to outstanding/{{id}} to delete.",
            to
        )))
    }
}

#[cfg(test)]
#[path = "background_broker_tests.rs"]
mod tests;
