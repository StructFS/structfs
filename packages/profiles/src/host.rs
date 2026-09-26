use crate::*;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use structfs_core_store::{DetachedFuture, Error, NoCodec, Record, Value};
use structfs_handles::{CancelToken, Gate};
use structfs_serde_store::{from_value, to_value};
use structfs_service::{
    CallContext, Operation, OwnerHandle, Registration, ResourceKind, Response, Service,
};

/// Decorate a provider with read-only discovery at `meta/profiles`.
pub struct Profiled {
    provider: Arc<dyn Service>,
    declarations: Vec<Declaration>,
}
impl Profiled {
    /// Wrap the result in `Arc` to mount it.
    pub fn new(provider: Arc<dyn Service>, declarations: Vec<Declaration>) -> Result<Self, Error> {
        let mut seen = BTreeSet::new();
        if declarations.len() > 16
            || declarations
                .iter()
                .any(|d| d.version != 1 || !seen.insert(d.profile))
        {
            return Err(Error::invalid_argument("invalid profile declarations"));
        }
        Ok(Self {
            provider,
            declarations,
        })
    }
}
impl Service for Profiled {
    fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
        if op.path().to_string() == "meta/profiles" {
            let result = c.ensure_active().and_then(|()| match op {
                Operation::Read(_) => {
                    to_value(&self.declarations).map(|v| Response::Read(Some(Record::parsed(v))))
                }
                _ => Err(Error::permission_denied("profile discovery is read only")),
            });
            return Box::pin(async move {
                let _context = c;
                result
            });
        }
        self.provider.call(c, op)
    }
}
#[derive(Default)]
pub struct HeadlessHost {
    surfaces: Mutex<BTreeMap<String, String>>,
}
struct Queue {
    events: VecDeque<InputEnvelope>,
    bytes: usize,
    in_flight: Option<InputEnvelope>,
    status: SessionStatus,
    input_closed: bool,
}
impl Queue {
    /// The single definition of "is there an event this reducer may take".
    ///
    /// Both the direct API and the mounted service go through this, so a
    /// closed session cannot hand out an event through one path while the
    /// other reports it closed.
    fn take_next(&mut self) -> Option<Result<InputEnvelope, Error>> {
        if self.status.closed {
            return Some(Err(Error::cancelled("session closed")));
        }
        if self.in_flight.is_some() {
            return Some(Err(Error::conflict("input awaits processing")));
        }
        self.events.pop_front().map(|input| {
            self.in_flight = Some(input.clone());
            Ok(input)
        })
    }
}
struct Inner {
    queue: Mutex<Queue>,
    gate: Gate,
}
pub struct Session {
    inner: Arc<Inner>,
    registration: Registration,
    max_events: usize,
    max_bytes: usize,
}
impl HeadlessHost {
    /// Surface keys are host capabilities, not diagnostic installation labels.
    ///
    /// Wrap the result in `Arc` to mount it as a service.
    pub fn open(
        self: &Arc<Self>,
        owner: &OwnerHandle,
        surface: &str,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<Session, Error> {
        if max_events == 0 || max_bytes < 64 || surface.len() > 256 {
            return Err(Error::invalid_argument("session bounds"));
        }
        let mut surfaces = self.surfaces.lock().unwrap_or_else(|e| e.into_inner());
        if surfaces.contains_key(surface) {
            return Err(Error::conflict("presentation surface already owned"));
        }
        let id = format!("s{}", uuid::Uuid::new_v4().simple());
        let inner = Arc::new(Inner {
            queue: Mutex::new(Queue {
                events: VecDeque::new(),
                bytes: 0,
                in_flight: None,
                input_closed: false,
                status: SessionStatus {
                    session: id.clone(),
                    accepted: 0,
                    processed: 0,
                    rendered: None,
                    closed: false,
                },
            }),
            gate: Gate::new(),
        });
        let host = Arc::downgrade(self);
        let cleanup = inner.clone();
        let key = surface.to_string();
        let identity = id.clone();
        let registration =
            owner.register(ResourceKind::Registration, max_bytes, move || async move {
                {
                    let mut q = cleanup.queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.events.clear();
                    q.in_flight = None;
                    q.bytes = 0;
                    q.status.closed = true;
                }
                cleanup.gate.notify();
                if let Some(host) = host.upgrade() {
                    let mut s = host.surfaces.lock().unwrap_or_else(|e| e.into_inner());
                    if s.get(&key) == Some(&identity) {
                        s.remove(&key);
                    }
                }
                Ok(())
            })?;
        surfaces.insert(surface.into(), id);
        Ok(Session {
            inner,
            registration,
            max_events,
            max_bytes,
        })
    }
}
impl Session {
    pub fn status(&self) -> SessionStatus {
        let mut status = self
            .inner
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone();
        status.closed |= self.registration.cancellation().is_cancelled();
        status
    }
    pub fn submit(&self, input: InputEnvelope) -> Result<(), Error> {
        let mut q = self.inner.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.input_closed || self.registration.cancellation().is_cancelled() {
            return Err(Error::cancelled("session input closed"));
        }
        input
            .validate(&q.status.session, q.status.accepted)
            .map_err(Error::invalid_argument)?;
        // Reject anything the wire form cannot carry, before it is queued.
        to_value(&input)?;
        let bytes = input
            .weight()
            .ok_or_else(|| Error::resource_limit("input weight"))?;
        if q.events.len() + usize::from(q.in_flight.is_some()) >= self.max_events
            || bytes > self.max_bytes.saturating_sub(q.bytes)
        {
            return Err(Error::overloaded("input queue full"));
        }
        q.status.accepted = input.sequence;
        q.bytes += bytes;
        q.input_closed = matches!(input.input, Input::Close {});
        q.events.push_back(input);
        drop(q);
        self.inner.gate.notify();
        Ok(())
    }
    /// Consuming read; at most one reducer may hold an unacknowledged event.
    pub async fn next(&self, cancel: &CancelToken) -> Result<InputEnvelope, Error> {
        let released = self.registration.cancellation();
        let read = self.inner.gate.wait_until_cancellable(cancel, || {
            self.inner
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take_next()
        });
        tokio::select! { biased;
            _ = released.cancelled() => Err(Error::cancelled("session closed")),
            result = read => result.map_err(|e| e.into_error("input cancelled"))?,
        }
    }
    /// Acknowledge only after the reducer has committed its state/effect intent.
    pub fn processed(&self, sequence: u64) -> Result<(), Error> {
        let mut q = self.inner.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.in_flight.as_ref().map(|e| e.sequence) != Some(sequence) {
            return Err(Error::invalid_argument("wrong processed sequence"));
        }
        let input = q.in_flight.take().unwrap();
        q.bytes -= input.weight().unwrap();
        q.status.processed = sequence;
        drop(q);
        self.inner.gate.notify();
        Ok(())
    }
    pub fn presented(&self, token: Token) -> Result<(), Error> {
        if token.epoch.len() > 128 {
            return Err(Error::invalid_argument("epoch size"));
        }
        if self.registration.cancellation().is_cancelled() {
            return Err(Error::cancelled("session closed"));
        }
        let mut q = self.inner.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.status
            .rendered
            .as_ref()
            .is_some_and(|old| old.epoch != token.epoch || old.revision > token.revision)
        {
            return Err(Error::conflict("stale presentation"));
        }
        q.status.rendered = Some(token);
        Ok(())
    }
    /// Request teardown of the session. Non-blocking; queued input is dropped
    /// and parked reads wake.
    pub fn close(&self) {
        self.registration.close();
    }
    /// [`Session::close`], then wait at most `timeout` for the teardown, and
    /// report the owner's remaining resources.
    pub async fn join(&self, timeout: std::time::Duration) -> structfs_service::CloseReport {
        self.registration.join(timeout).await
    }
}
impl Service for Session {
    fn call(&self, c: CallContext, op: Operation) -> DetachedFuture<Response> {
        // State-changing work here is short and synchronous; parked input reads
        // retain their call context and never hold the queue mutex while waiting.
        let inner = self.inner.clone();
        let released = self.registration.cancellation();
        match op {
            Operation::Read(p) if p.to_string() == "input/next" => Box::pin(async move {
                c.ensure_active()?;
                let _lease = c.lease();
                let read = inner.gate.wait_until_cancellable(&c.cancellation, || {
                    inner
                        .queue
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .take_next()
                });
                let input = tokio::select! { biased;
                    _ = released.cancelled() => Err(Error::cancelled("session closed")),
                    result = read => result.map_err(|e| e.into_error("input cancelled"))?,
                }?;
                Ok(Response::Read(Some(Record::parsed(to_value(&input)?))))
            }),
            op => {
                let result = c.ensure_active().and_then(|()| match op {
                    Operation::Read(p) if p.to_string() == "status" => {
                        to_value(&self.status()).map(|v| Response::Read(Some(Record::parsed(v))))
                    }
                    Operation::Write(p, r) => {
                        let value = r.into_value(&NoCodec)?;
                        match p.to_string().as_str() {
                            "input" => self.submit(from_value(value)?)?,
                            "processed" => self.processed(from_value(value)?)?,
                            "presented" => self.presented(from_value(value)?)?,
                            "release" if value == Value::Null => self.close(),
                            _ => return Err(Error::permission_denied("interactive path")),
                        }
                        Ok(Response::Written(p))
                    }
                    _ => Err(Error::permission_denied("interactive path")),
                });
                Box::pin(async move {
                    let _context = c;
                    result
                })
            }
        }
    }
}
