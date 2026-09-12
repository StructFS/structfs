use crate::{OperationStatus, Phase};
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use structfs_core_store::{DetachedFuture, Error, Record, Value};
use structfs_serde_store::to_value;
use structfs_service::{
    CallContext, CancelToken, Lease, Operation, OwnerHandle, Registration, ResourceId,
    ResourceKind, Response, Service,
};

struct State {
    phase: Phase,
    result: Option<Vec<u8>>,
    released: bool,
    charge: Option<Lease>,
}
struct Inner {
    state: Mutex<State>,
    cancel: CancelToken,
}
/// A single bounded operation. The granting provider chooses its returned path.
/// Dropping/releasing this handle requests cancellation; the owner joins work.
pub struct OperationHandle {
    id: String,
    inner: Arc<Inner>,
    owner: OwnerHandle,
    task: ResourceId,
    registration: Registration,
}
struct Completion(Arc<Inner>);
impl Drop for Completion {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(state.phase, Phase::Pending | Phase::Running) {
            state.phase = Phase::Failed;
        }
    }
}
impl OperationHandle {
    /// Admission and retained-result capacity are reserved before invoking work.
    /// The callback must bound its own working memory and observe cancellation.
    pub fn start<F, Fut>(
        owner: &OwnerHandle,
        id: String,
        max_result_bytes: usize,
        work: F,
    ) -> Result<Arc<Self>, Error>
    where
        F: FnOnce(CancelToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Vec<u8>, Error>> + Send + 'static,
    {
        if id.is_empty() || id.len() > 128 || max_result_bytes == 0 {
            return Err(Error::resource_limit("operation bounds"));
        }
        let charge = Lease::new(owner.track(ResourceKind::Registration, max_result_bytes)?);
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                phase: Phase::Pending,
                result: None,
                released: false,
                charge: Some(charge.clone()),
            }),
            cancel: CancelToken::new(),
        });
        let cleanup = inner.clone();
        let registration = owner.register(ResourceKind::Registration, 0, move || async move {
            cleanup.cancel.cancel();
            let mut state = cleanup.state.lock().unwrap_or_else(|e| e.into_inner());
            state.released = true;
            state.result = None;
            state.charge.take();
            Ok(())
        })?;
        let running = inner.clone();
        let task = owner.spawn(move |owner_cancel| async move {
            let _charge = charge;
            let _completion = Completion(running.clone());
            running
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .phase = Phase::Running;
            let future = async { work(running.cancel.clone()).await };
            tokio::pin!(future);
            let result = tokio::select! { biased;
                _ = owner_cancel.cancelled() => { running.cancel.cancel(); future.await },
                result = &mut future => result,
            };
            let mut state = running.state.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Ok(bytes) if bytes.len() <= max_result_bytes => {
                    state.phase = Phase::Completed;
                    if !state.released {
                        state.result = Some(bytes);
                    }
                }
                _ => state.phase = Phase::Failed,
            }
            // Application failure is a terminal outcome, not a cleanup failure.
            Ok(())
        })?;
        Ok(Arc::new(Self {
            id,
            inner,
            owner: owner.clone(),
            task,
            registration,
        }))
    }
    pub fn status(&self) -> OperationStatus {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let joined = !self
            .owner
            .report()
            .remaining
            .iter()
            .any(|r| r.id == self.task && !r.failed);
        OperationStatus {
            operation: self.id.clone(),
            phase: state.phase.clone(),
            cancel_requested: self.inner.cancel.is_cancelled()
                || self.owner.cancellation().is_cancelled(),
            joined,
            result_bytes: state.result.as_ref().map_or(0, Vec::len),
        }
    }
    pub fn result(&self) -> Result<Option<Vec<u8>>, Error> {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.released || self.registration.cancellation().is_cancelled() {
            return Err(Error::cancelled("operation released"));
        }
        if matches!(state.phase, Phase::Failed) {
            return Err(Error::store("operation", "result", "operation failed"));
        }
        Ok(state.result.clone())
    }
    pub fn cancel(&self) {
        self.inner.cancel.cancel();
    }
    pub fn release(&self) {
        self.cancel();
        self.registration.release();
    }
}
impl Service for OperationHandle {
    fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
        let result = c.ensure_active().and_then(|()| match op {
            Operation::Read(p) if p.to_string() == "status" => {
                to_value(&self.status()).map(|v| Response::Read(Some(Record::parsed(v))))
            }
            Operation::Read(p) if p.to_string() == "result" => self
                .result()
                .map(|v| Response::Read(v.map(|b| Record::parsed(Value::Bytes(b))))),
            Operation::Write(p, r) => {
                if r.into_value(&structfs_core_store::NoCodec)? != Value::Null {
                    return Err(Error::conflict("operation control expects null"));
                }
                match p.to_string().as_str() {
                    "cancel" => self.cancel(),
                    "release" => self.release(),
                    _ => return Err(Error::permission_denied("operation path")),
                }
                Ok(Response::Written(p))
            }
            _ => Err(Error::permission_denied("operation path")),
        });
        Box::pin(async move {
            let _context = c;
            result
        })
    }
}
