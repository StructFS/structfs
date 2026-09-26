//! The background HTTP broker's handle protocol, on `structfs-handles`.
//!
//! [`crate::BackgroundHttpBrokerStore`] is a thin sync facade (in
//! `background_broker.rs`) over a [`HandleStore`] running this protocol. The
//! scaffolding — id minting, `outstanding/{id}` routing, the no-overwrite
//! rule, Null-write release with cancellation, listing — comes from the
//! handles crate; this module only defines what an HTTP request handle *is*:
//! queued request, status, response, and a parked `response/wait` read that
//! cancels on release instead of sleep-polling.
//!
//! Requests run through the same [`BlockingHttpExecutor`] seam the other
//! stores use, one blocking call per background thread.

use std::sync::{Arc, Mutex};

use structfs_core_store::{DetachedFuture, Error, Path, Record, Value};
use structfs_handles::{CancelToken, Gate, HandleCx, HandleProtocol};
use structfs_serde_store::{from_value, to_value};

use crate::broker_common::{navigate, CachedFailure};
use crate::executor::BlockingHttpExecutor;
use crate::handle::RequestStatus;
use crate::types::{HttpRequest, HttpResponse};

struct HandleState {
    status: RequestStatus,
    response: Option<HttpResponse>,
    failure: Option<CachedFailure>,
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
        state.response.is_some() || state.failure.is_some()
    }
}

/// How the protocol executes requests.
pub(crate) enum Execution<E> {
    /// Spawn a thread per request running the blocking executor.
    Threaded(Arc<E>),
    /// Never execute — handles stay pending forever. Test-only: makes
    /// parked-read cancellation deterministic.
    #[cfg(test)]
    Never,
}

/// The handle protocol for the background HTTP broker.
pub(crate) struct HttpBrokerProtocol<E: BlockingHttpExecutor + 'static> {
    pub(crate) execution: Execution<E>,
}

impl<E: BlockingHttpExecutor + 'static> HandleProtocol for HttpBrokerProtocol<E> {
    type Handle = HttpHandle;

    fn open(&self, cx: HandleCx, request_value: Value) -> Result<Self::Handle, Error> {
        let request: HttpRequest = from_value(request_value)
            .map_err(|e| Error::invalid_argument(format!("Data must be an HttpRequest: {e}")))?;

        let shared = Arc::new(Shared {
            state: Mutex::new(HandleState {
                status: RequestStatus::pending(cx.id.to_string()),
                response: None,
                failure: None,
            }),
            gate: Gate::new(),
        });

        match &self.execution {
            Execution::Threaded(executor) => {
                let worker_shared = shared.clone();
                let worker_request = request.clone();
                let executor = executor.clone();
                let id = cx.id;
                std::thread::spawn(move || {
                    let result = executor.execute(&worker_request);
                    {
                        let mut state = worker_shared.lock();
                        match result {
                            Ok(response) => {
                                state.status = RequestStatus::complete(id.to_string());
                                state.response = Some(response);
                            }
                            Err(error) => {
                                let failure = CachedFailure::new(error);
                                state.status =
                                    RequestStatus::failed(id.to_string(), failure.message().into());
                                state.failure = Some(failure);
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
            // outstanding/{id} — status snapshot
            if sub.is_empty() {
                let value = to_value(&handle.status())
                    .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
                return Ok(Some(Record::parsed(value)));
            }

            // outstanding/{id}/request[/...]
            if &sub[0] == "request" {
                let value = to_value(handle.request())
                    .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
                return Ok(navigate(value, &sub.slice(1, sub.len())).map(Record::parsed));
            }

            if &sub[0] == "response" {
                // outstanding/{id}/response/wait[/...] — parked read: no
                // sleep-polling, and release cancels it (the read fails,
                // the caller unwinds).
                let nav_start = if sub.len() > 1 && &sub[1] == "wait" {
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
                    return Ok(
                        navigate(value, &sub.slice(nav_start, sub.len())).map(Record::parsed)
                    );
                }
                if let Some(ref failure) = state.failure {
                    return Err(failure.to_error());
                }
                // Non-blocking read of a pending response.
                return Ok(None);
            }

            Err(Error::invalid_argument(format!(
                "Unknown sub-path '{}'. Use 'request', 'response', or 'response/wait'.",
                &sub[0]
            )))
        })
    }

    fn write(&self, _handle: Arc<Self::Handle>, sub: Path, _data: Record) -> DetachedFuture<Path> {
        Box::pin(async move {
            Err(Error::invalid_argument(format!(
                "Invalid write path 'outstanding/{{id}}/{}'. Write to root to queue a \
                 request, or write null to outstanding/{{id}} to delete.",
                sub
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::mock::MockExecutor;
    use std::time::Duration;
    use structfs_core_store::{path, DetachedReader, DetachedWriter};
    use structfs_handles::HandleStore;

    fn pending_store() -> HandleStore<HttpBrokerProtocol<MockExecutor>> {
        HandleStore::new(HttpBrokerProtocol {
            execution: Execution::Never,
        })
    }

    fn executing_store(executor: MockExecutor) -> HandleStore<HttpBrokerProtocol<MockExecutor>> {
        HandleStore::new(HttpBrokerProtocol {
            execution: Execution::Threaded(Arc::new(executor)),
        })
    }

    fn request_value() -> Value {
        to_value(&HttpRequest::get("https://api.test/thing")).unwrap()
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

    #[tokio::test]
    async fn executed_requests_go_through_the_executor_seam() {
        let executor = MockExecutor::new().with_response(
            "https://api.test/thing",
            MockExecutor::success_response(serde_json::json!({"ok": true})),
        );
        let mut store = executing_store(executor.clone());
        let handle = store
            .write_detached(&path!(""), Record::parsed(request_value()))
            .await
            .unwrap();

        let record = store
            .read_detached(&handle.join(&path!("response/wait")))
            .await
            .unwrap()
            .unwrap();
        let response: HttpResponse = from_value(record.as_value().unwrap().clone()).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, serde_json::json!({"ok": true}));

        assert_eq!(executor.recorded_requests().len(), 1);
    }

    #[tokio::test]
    async fn executor_failures_surface_as_typed_errors() {
        let mut store = executing_store(MockExecutor::new().fail_with("Connection refused"));
        let handle = store
            .write_detached(&path!(""), Record::parsed(request_value()))
            .await
            .unwrap();

        let error = store
            .read_detached(&handle.join(&path!("response/wait")))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Connection refused"), "{error}");

        // The status snapshot reports the same failure.
        let record = store.read_detached(&handle).await.unwrap().unwrap();
        let status: RequestStatus = from_value(record.as_value().unwrap().clone()).unwrap();
        assert!(status.is_failed());
        assert!(status.error.unwrap().contains("Connection refused"));
    }

    #[tokio::test]
    async fn missing_response_fields_read_as_absent() {
        let executor = MockExecutor::new().with_response(
            "https://api.test/thing",
            MockExecutor::success_response(serde_json::json!({"a": 1})),
        );
        let mut store = executing_store(executor);
        let handle = store
            .write_detached(&path!(""), Record::parsed(request_value()))
            .await
            .unwrap();

        store
            .read_detached(&handle.join(&path!("response/wait")))
            .await
            .unwrap();
        let missing = store
            .read_detached(&handle.join(&path!("response/body/nope")))
            .await
            .unwrap();
        assert!(missing.is_none());
    }
}
