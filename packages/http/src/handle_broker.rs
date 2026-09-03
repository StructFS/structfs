//! The async HTTP broker's handle protocol, on `structfs-handles`.
//!
//! `AsyncHttpBrokerStore` is a thin sync facade (in `core.rs`) over a
//! [`HandleStore`] running this protocol. The scaffolding — id minting,
//! `outstanding/{id}` routing, the no-overwrite rule, Null-write release
//! with cancellation, listing — comes from the handles crate; this module
//! only defines what an HTTP request handle *is*: queued request, status,
//! response, and a parked `response/wait` read that cancels on release
//! instead of sleep-polling.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use structfs_core_store::{DetachedFuture, Error, Path, Record, Value};
use structfs_handles::{CancelToken, Gate, HandleCx, HandleProtocol};
use structfs_serde_store::{from_value, to_value};

use crate::handle::RequestStatus;
use crate::types::{HttpRequest, HttpResponse};

/// Navigate into a Value structure using path components.
fn navigate(value: Value, path: &[String]) -> Result<Value, Error> {
    let mut current = value;
    for (i, key) in path.iter().enumerate() {
        current = match current {
            Value::Map(map) => map
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .ok_or_else(|| {
                    Error::store(
                        "async_http_broker",
                        "read",
                        format!("Path not found at index {}: '{}'", i, key),
                    )
                })?,
            Value::Array(arr) => {
                let index: usize = key.parse().map_err(|_| {
                    Error::store(
                        "async_http_broker",
                        "read",
                        format!("Path not found at index {}: '{}'", i, key),
                    )
                })?;
                arr.into_iter().nth(index).ok_or_else(|| {
                    Error::store(
                        "async_http_broker",
                        "read",
                        format!("Path not found at index {}: '{}'", i, key),
                    )
                })?
            }
            _ => {
                return Err(Error::store(
                    "async_http_broker",
                    "read",
                    format!("Path not found at index {}: '{}'", i, key),
                ))
            }
        };
    }
    Ok(current)
}

struct HandleState {
    status: RequestStatus,
    response: Option<HttpResponse>,
}

struct Shared {
    state: Mutex<HandleState>,
    gate: Gate,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, HandleState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Per-request handle state.
pub(crate) struct HttpHandle {
    request: HttpRequest,
    shared: Arc<Shared>,
    cancel: CancelToken,
}

impl HttpHandle {
    /// The queued request.
    pub(crate) fn request(&self) -> &HttpRequest {
        &self.request
    }

    /// A snapshot of the request status.
    pub(crate) fn status(&self) -> RequestStatus {
        self.shared.lock().status.clone()
    }

    /// Whether the request finished (successfully or not).
    pub(crate) fn is_settled(&self) -> bool {
        let state = self.shared.lock();
        state.response.is_some() || state.status.is_failed()
    }
}

/// How the protocol executes requests.
pub(crate) enum Execution {
    /// Spawn a thread per request running the blocking HTTP client.
    Threaded {
        timeout: Duration,
        execute: fn(HttpRequest, Duration) -> Result<HttpResponse, String>,
    },
    /// Never execute — handles stay pending forever. Test-only: makes
    /// parked-read cancellation deterministic.
    #[cfg(test)]
    Never,
}

/// The handle protocol for the async HTTP broker.
pub(crate) struct HttpBrokerProtocol {
    pub(crate) execution: Execution,
}

impl HandleProtocol for HttpBrokerProtocol {
    type Handle = HttpHandle;

    fn open(&self, cx: HandleCx, request_value: Value) -> Result<Self::Handle, Error> {
        let request: HttpRequest = from_value(request_value).map_err(|e| {
            Error::decode(
                structfs_core_store::Format::JSON,
                format!("Data must be an HttpRequest: {}", e),
            )
        })?;

        let shared = Arc::new(Shared {
            state: Mutex::new(HandleState {
                status: RequestStatus::pending(cx.id.to_string()),
                response: None,
            }),
            gate: Gate::new(),
        });

        match self.execution {
            Execution::Threaded { timeout, execute } => {
                let worker_shared = shared.clone();
                let worker_request = request.clone();
                let id = cx.id;
                std::thread::spawn(move || {
                    let result = execute(worker_request, timeout);
                    {
                        let mut state = worker_shared.lock();
                        match result {
                            Ok(response) => {
                                state.status = RequestStatus::complete(id.to_string());
                                state.response = Some(response);
                            }
                            Err(error) => {
                                state.status = RequestStatus::failed(id.to_string(), error);
                            }
                        }
                    }
                    worker_shared.gate.notify();
                });
            }
            #[cfg(test)]
            Execution::Never => {}
        }

        Ok(HttpHandle {
            request,
            shared,
            cancel: cx.cancel,
        })
    }

    fn read(&self, handle: Arc<Self::Handle>, sub: Path) -> DetachedFuture<Option<Record>> {
        Box::pin(async move {
            let components: Vec<String> = sub.iter().cloned().collect();

            // outstanding/{id} — status snapshot
            if components.is_empty() {
                let value = to_value(&handle.status())
                    .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
                return Ok(Some(Record::parsed(value)));
            }

            // outstanding/{id}/request[/...]
            if components[0] == "request" {
                let value = to_value(handle.request())
                    .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
                let value = navigate(value, &components[1..])?;
                return Ok(Some(Record::parsed(value)));
            }

            if components[0] == "response" {
                // outstanding/{id}/response/wait[/...] — parked read: no
                // sleep-polling, and release cancels it (the read fails,
                // the caller unwinds).
                let nav_start = if components.get(1).map(String::as_str) == Some("wait") {
                    handle
                        .shared
                        .gate
                        .wait_until_cancellable(&handle.cancel, || {
                            handle.is_settled().then_some(())
                        })
                        .await
                        .map_err(|c| c.into_error("handle released while waiting"))?;
                    2
                } else {
                    1
                };

                let state = handle.shared.lock();
                if let Some(ref response) = state.response {
                    let value = to_value(response).map_err(|e| {
                        Error::encode(structfs_core_store::Format::JSON, e.to_string())
                    })?;
                    drop(state);
                    let value = navigate(value, &components[nav_start..])?;
                    return Ok(Some(Record::parsed(value)));
                }
                if state.status.is_failed() {
                    return Err(Error::store(
                        "async_http_broker",
                        "read",
                        format!(
                            "HTTP request failed: {}",
                            state.status.error.as_deref().unwrap_or("unknown error")
                        ),
                    ));
                }
                // Non-blocking read of a pending response.
                return Ok(None);
            }

            Err(Error::store(
                "async_http_broker",
                "read",
                format!(
                    "Unknown sub-path '{}'. Use 'request', 'response', or 'response/wait'.",
                    components[0]
                ),
            ))
        })
    }

    fn write(&self, _handle: Arc<Self::Handle>, sub: Path, _data: Record) -> DetachedFuture<Path> {
        Box::pin(async move {
            Err(Error::store(
                "async_http_broker",
                "write",
                format!(
                    "Invalid write path 'outstanding/{{id}}/{}'. Write to root to queue a \
                     request, or write null to outstanding/{{id}} to delete.",
                    sub
                ),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, DetachedReader, DetachedWriter};
    use structfs_handles::HandleStore;

    fn pending_store() -> HandleStore<HttpBrokerProtocol> {
        HandleStore::new(HttpBrokerProtocol {
            execution: Execution::Never,
        })
    }

    fn request_value() -> Value {
        to_value(&HttpRequest::get("https://example.com/test")).unwrap()
    }

    #[tokio::test]
    async fn broker_protocol_passes_handle_conformance() {
        let mut store = pending_store();
        structfs_handles::conformance::check_handle_conventions(&mut store, request_value()).await;
    }

    #[tokio::test]
    async fn parked_wait_is_cancelled_by_release() {
        let mut store = pending_store();
        let handle = store
            .write_detached(&path!(""), Record::parsed(request_value()))
            .await
            .unwrap();

        // Park a response/wait on the never-completing request.
        let mut reader = store.clone();
        let wait_path = handle.join(&path!("response/wait"));
        let parked = tokio::spawn(async move { reader.read_detached(&wait_path).await });
        tokio::task::yield_now().await;

        // Release the handle: the parked read fails with Cancelled
        // instead of sleep-polling a deleted entry forever.
        store
            .write_detached(&handle, Record::parsed(Value::Null))
            .await
            .unwrap();
        let err = tokio::time::timeout(Duration::from_secs(5), parked)
            .await
            .expect("parked wait never resolved")
            .unwrap()
            .unwrap_err();
        assert!(err.is_cancelled());
    }

    #[tokio::test]
    async fn pending_response_reads_absent_without_blocking() {
        let mut store = pending_store();
        let handle = store
            .write_detached(&path!(""), Record::parsed(request_value()))
            .await
            .unwrap();

        let response = store
            .read_detached(&handle.join(&path!("response")))
            .await
            .unwrap();
        assert!(response.is_none());

        // Status is pending.
        let status = store.read_detached(&handle).await.unwrap().unwrap();
        let status: RequestStatus = from_value(status.as_value().unwrap().clone()).unwrap();
        assert!(!status.is_failed());
    }
}
