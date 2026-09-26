//! The blocking HTTP broker: write queues a request, read executes it.

use std::collections::BTreeMap;
use std::time::Duration;

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};
use structfs_serde_store::{from_value, to_value};

use crate::broker_common::{
    broker_docs, navigate, outstanding_listing, outstanding_path, read_meta, root_references,
    BrokerKind, CachedFailure, HandleMeta, RequestId, DOCS_PATH, META_PATH, OUTSTANDING_PREFIX,
};
use crate::executor::{BlockingHttpExecutor, BlockingReqwestExecutor};
use crate::types::{HttpRequest, HttpResponse};

/// State of a request handle in the sync broker.
///
/// A handle is queued, then executed exactly once; `outcome` holds the
/// cached result, so every later read returns the same response or the same
/// failure. There is no "executed but empty" state to guard against.
struct SyncRequestHandle {
    request: HttpRequest,
    outcome: Option<Result<HttpResponse, CachedFailure>>,
}

impl SyncRequestHandle {
    fn new(request: HttpRequest) -> Self {
        Self {
            request,
            outcome: None,
        }
    }

    fn meta(&self) -> HandleMeta {
        HandleMeta {
            status: match &self.outcome {
                None => "pending",
                Some(Ok(_)) => "complete",
                Some(Err(_)) => "failed",
            },
            method: format!("{:?}", self.request.method),
            url: self.request.path.clone(),
        }
    }
}

/// HTTP broker store for sync (blocking) requests.
///
/// Write requests are queued and executed when reading from the handle path.
/// **Reads are idempotent**: the first read executes the request and caches the result;
/// subsequent reads return the cached response.
///
/// ## Path Structure
///
/// | Path | Operation | Result |
/// |------|-----------|--------|
/// | `write /` | Queue request | Returns `outstanding/{id}` |
/// | `read /outstanding` | List handles | Returns `{items: [references]}` |
/// | `read /outstanding/{id}` | Execute (blocks) & return response | Returns cached response |
/// | `read /outstanding/{id}/response` | Same as above | Returns cached response |
/// | `read /outstanding/{id}/request` | View queued request | Returns original request |
/// | `write /outstanding/{id} null` | Delete handle | Removes handle |
///
/// Unknown or deleted handles read as absent (`Ok(None)`), like any other
/// missing path.
///
/// Generic over the HTTP executor to allow mocking in tests.
///
/// # Runtime constraint
///
/// The default executor builds a blocking reqwest client, which **panics
/// when constructed inside a Tokio runtime**. Build this store on an
/// ordinary thread, or supply your own executor with
/// [`HttpBrokerStore::with_executor`].
///
/// # Examples
///
/// ```no_run
/// use structfs_core_store::{path, Reader, Record, Writer};
/// use structfs_http::{HttpBrokerStore, HttpRequest};
/// use structfs_serde_store::to_value;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut broker = HttpBrokerStore::with_default_timeout()?;
///
/// // Queue a request; the returned path is the handle.
/// let request = HttpRequest::get("https://example.com");
/// let handle = broker.write(&path!(""), Record::parsed(to_value(&request)?))?;
///
/// // Reading the handle executes it and caches the response.
/// let response = broker.read(&handle)?;
/// # Ok(())
/// # }
/// ```
pub struct HttpBrokerStore<E: BlockingHttpExecutor = BlockingReqwestExecutor> {
    handles: BTreeMap<RequestId, SyncRequestHandle>,
    next_request_id: RequestId,
    executor: E,
}

impl HttpBrokerStore<BlockingReqwestExecutor> {
    /// Create a new HTTP broker store with the given request timeout.
    ///
    /// # Panics
    ///
    /// Panics if called from within a Tokio runtime; see the type docs.
    pub fn new(timeout: Duration) -> Result<Self, crate::Error> {
        Ok(Self::with_executor(BlockingReqwestExecutor::new(timeout)?))
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

impl<E: BlockingHttpExecutor> HttpBrokerStore<E> {
    /// Create a new HTTP broker store with a custom executor.
    ///
    /// This is also how tests substitute a mock executor.
    pub fn with_executor(executor: E) -> Self {
        Self {
            handles: BTreeMap::new(),
            next_request_id: 0,
            executor,
        }
    }

    /// The ids of the queued handles, ascending.
    pub fn handle_ids(&self) -> Vec<RequestId> {
        self.handles.keys().copied().collect()
    }

    /// Whether a handle is still queued.
    pub fn has_handle(&self, id: RequestId) -> bool {
        self.handles.contains_key(&id)
    }

    /// The number of queued handles.
    pub fn handle_count(&self) -> usize {
        self.handles.len()
    }

    /// Split `outstanding/{id}/...` into the id and the path below it.
    ///
    /// `None` for anything that is not a handle path, including the
    /// `outstanding` listing itself.
    fn parse_handle_path(path: &Path) -> Option<(RequestId, Path)> {
        if path.len() < 2 || &path[0] != OUTSTANDING_PREFIX {
            return None;
        }
        let id: RequestId = path[1].parse().ok()?;
        Some((id, path.slice(2, path.len())))
    }

    /// Execute the handle's request unless it already has an outcome.
    fn ensure_executed(&mut self, id: RequestId) {
        let Some(handle) = self.handles.get(&id) else {
            return;
        };
        if handle.outcome.is_some() {
            return;
        }
        let outcome = self
            .executor
            .execute(&handle.request)
            .map_err(CachedFailure::new);
        if let Some(handle) = self.handles.get_mut(&id) {
            handle.outcome = Some(outcome);
        }
    }
}

impl<E: BlockingHttpExecutor> Reader for HttpBrokerStore<E> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(root_references())));
        }

        if &from[0] == DOCS_PATH {
            return Ok(Some(Record::parsed(broker_docs(BrokerKind::Blocking))));
        }

        if &from[0] == META_PATH {
            let ids = self.handle_ids();
            return read_meta(BrokerKind::Blocking, from, &ids, |id| {
                self.handles.get(&id).map(SyncRequestHandle::meta)
            });
        }

        if from.len() == 1 && &from[0] == OUTSTANDING_PREFIX {
            return Ok(Some(Record::parsed(outstanding_listing(
                &self.handle_ids(),
            ))));
        }

        let (request_id, sub) = Self::parse_handle_path(from).ok_or_else(|| {
            Error::invalid_argument(format!(
                "Invalid path '{}'. Expected: outstanding, outstanding/{{id}}, or outstanding/{{id}}/request",
                from
            ))
        })?;

        if !self.handles.contains_key(&request_id) {
            return Ok(None);
        }

        // outstanding/{id}/request[/...] — view the queued request.
        if !sub.is_empty() && &sub[0] == "request" {
            let handle = &self.handles[&request_id];
            let value = to_value(&handle.request)
                .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
            return Ok(navigate(value, &sub.slice(1, sub.len())).map(Record::parsed));
        }

        // outstanding/{id}[/response[/...]] — execute and return the response.
        // Both spellings block, for symmetry with the background broker.
        if sub.is_empty() || &sub[0] == "response" {
            self.ensure_executed(request_id);
            let nav = if sub.is_empty() {
                sub.clone()
            } else {
                sub.slice(1, sub.len())
            };
            return match &self.handles[&request_id].outcome {
                Some(Ok(response)) => {
                    let value = to_value(response).map_err(|e| {
                        Error::encode(structfs_core_store::Format::JSON, e.to_string())
                    })?;
                    Ok(navigate(value, &nav).map(Record::parsed))
                }
                Some(Err(failure)) => Err(failure.to_error()),
                // `ensure_executed` always leaves an outcome behind for a
                // handle that exists, and the handle cannot vanish under a
                // `&mut self` read.
                None => Ok(None),
            };
        }

        Err(Error::invalid_argument(format!(
            "Unknown sub-path '{}'. Use 'request' or 'response'.",
            &sub[0]
        )))
    }
}

impl<E: BlockingHttpExecutor> Writer for HttpBrokerStore<E> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&NoCodec)?;

        // Handle writes, with the same answers as the background broker:
        // Null to outstanding/{id} deletes (idempotently); anything written
        // at or below a handle that does not exist is NotFound; a non-Null
        // overwrite of a live handle is Conflict; a write below a live
        // handle is InvalidArgument (handles have no writable sub-paths).
        if let Some((request_id, sub)) = Self::parse_handle_path(to) {
            if value == Value::Null && sub.is_empty() {
                self.handles.remove(&request_id);
                return Ok(to.clone());
            }
            if !self.handles.contains_key(&request_id) {
                return Err(Error::not_found(to.clone()));
            }
            if !sub.is_empty() {
                return Err(Error::invalid_argument(format!(
                    "Invalid write path '{}'. Write to root to queue a request, or write null to outstanding/{{id}} to delete.",
                    to
                )));
            }
            return Err(Error::conflict(
                "Cannot overwrite existing request. Write null to delete, or write to root to queue a new request.",
            ));
        }

        // Queue a new request: write to root.
        if to.is_empty() {
            let request: HttpRequest = from_value(value).map_err(|e| {
                Error::invalid_argument(format!("Data must be an HttpRequest: {e}"))
            })?;

            let request_id = self.next_request_id;
            self.next_request_id += 1;

            self.handles
                .insert(request_id, SyncRequestHandle::new(request));

            return Ok(outstanding_path(request_id));
        }

        Err(Error::invalid_argument(format!(
            "Invalid write path '{}'. Write to root to queue a request, or write null to outstanding/{{id}} to delete.",
            to
        )))
    }
}

#[cfg(test)]
#[path = "sync_broker_tests.rs"]
mod tests;
