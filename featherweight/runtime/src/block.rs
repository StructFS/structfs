//! Block identity, lifecycle state, and the per-block cell.
//!
//! A `BlockCell` is the single shared state record for one block instance:
//! lifecycle state, the server-protocol request queue, response
//! correlation, and shutdown flags. Everything that touches a block —
//! the runtime, its namespace, its `/iso/` surface, and callers routed to
//! its store — holds the same `Arc<BlockCell>`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use structfs_core_store::{Error, Path, Value};
use structfs_handles::{CancelToken, Gate};
use tokio::sync::oneshot;
use uuid::Uuid;

/// Unique block identifier, assigned by the runtime. Opaque to blocks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockId(String);

impl BlockId {
    /// Mint a fresh id.
    pub fn new() -> Self {
        Self(format!("block-{}", Uuid::new_v4()))
    }

    /// An id derived from a stable name — the assembly-scoped transcript
    /// key — so identity is a function of the assembly's shape, never of
    /// the run. `iso/self/id` then answers the same string every run,
    /// which the same-seed-same-run claim requires: an id is an input.
    pub fn named(key: &str) -> Self {
        Self(format!("block:{key}"))
    }

    /// The id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for BlockId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for BlockId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The six lifecycle states from
/// [spec 05](https://github.com/StructFS/structfs/blob/main/isotope/spec/05-lifecycle.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    Created,
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}

impl BlockState {
    /// Spec string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockState::Created => "created",
            BlockState::Starting => "starting",
            BlockState::Running => "running",
            BlockState::Stopping => "stopping",
            BlockState::Stopped => "stopped",
            BlockState::Failed => "failed",
        }
    }

    /// Whether the block will never process another request.
    pub fn is_terminal(&self) -> bool {
        matches!(self, BlockState::Stopped | BlockState::Failed)
    }
}

/// What happens to the assembly when this block fails
/// ([spec 02](https://github.com/StructFS/structfs/blob/main/isotope/spec/02-assemblies.md)
/// failure modes; restart is out of
/// scope for the strawman).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailurePolicy {
    /// Block failure fails the assembly.
    #[default]
    FailFast,
    /// Block failure is contained; its paths return `unavailable`.
    Isolate,
}

/// Shutdown mode
/// ([spec 05](https://github.com/StructFS/structfs/blob/main/isotope/spec/05-lifecycle.md)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownMode {
    Graceful,
    Immediate,
}

impl ShutdownMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ShutdownMode::Graceful => "graceful",
            ShutdownMode::Immediate => "immediate",
        }
    }
}

/// A server-protocol request queued for a block
/// ([spec 07](https://github.com/StructFS/structfs/blob/main/isotope/spec/07-server-protocol.md)).
#[derive(Debug, Clone)]
pub struct ServerRequest {
    /// `"read"` or `"write"`.
    pub op: &'static str,
    /// Path relative to the block's store root.
    pub path: Path,
    /// Data for writes; `Value::Null` for reads.
    pub data: Value,
    /// Correlation token; the block responds by writing to
    /// `iso/server/responses/{token}`.
    pub token: u64,
}

impl ServerRequest {
    /// Encode as the spec's request envelope.
    pub fn to_value(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert("op".to_string(), Value::from(self.op));
        map.insert("path".to_string(), Value::String(self.path.to_string()));
        map.insert("data".to_string(), self.data.clone());
        map.insert(
            "respond_to".to_string(),
            Value::String(format!("iso/server/responses/{}", self.token)),
        );
        Value::Map(map)
    }
}

/// One event on the block's unified mailbox
/// ([spec 09](https://github.com/StructFS/structfs/blob/main/isotope/spec/09-posix-closure.md)):
/// served requests interleaved
/// with runtime notifications.
#[derive(Debug, Clone)]
pub enum BlockEvent {
    /// A server-protocol request (carries `respond_to`).
    Request(ServerRequest),
    /// A runtime- or host-originated signal. Fire-and-forget.
    Signal { name: String, data: Value },
    /// A delivery for a timer the block registered.
    Timer { tag: Value },
}

impl BlockEvent {
    /// Encode as the mailbox envelope; `op` distinguishes event kinds.
    pub fn to_value(&self) -> Value {
        match self {
            BlockEvent::Request(request) => request.to_value(),
            BlockEvent::Signal { name, data } => {
                let mut map = std::collections::BTreeMap::new();
                map.insert("op".to_string(), Value::from("signal"));
                map.insert("signal".to_string(), Value::String(name.clone()));
                map.insert("data".to_string(), data.clone());
                Value::Map(map)
            }
            BlockEvent::Timer { tag } => {
                let mut map = std::collections::BTreeMap::new();
                map.insert("op".to_string(), Value::from("timer"));
                map.insert("tag".to_string(), tag.clone());
                Value::Map(map)
            }
        }
    }
}

/// Owns queue/correlation cleanup when a caller finishes, times out, or drops.
pub(crate) struct PendingCall {
    cell: std::sync::Arc<BlockCell>,
    token: u64,
    receiver: oneshot::Receiver<Value>,
    _charge: crate::admission::CallCharge,
}
impl std::future::Future for PendingCall {
    type Output = Result<Value, oneshot::error::RecvError>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.receiver).poll(cx)
    }
}
impl Drop for PendingCall {
    fn drop(&mut self) {
        let mut cell = self.cell.lock();
        cell.responses.remove(&self.token);
        cell.callers.remove(&self.token);
        cell.queue.retain(
            |event| !matches!(event, BlockEvent::Request(request) if request.token == self.token),
        );
    }
}

struct ShutdownFlags {
    requested: bool,
    mode: Option<ShutdownMode>,
    complete: bool,
    exit_code: Option<i64>,
}

struct CellState {
    state: BlockState,
    queue: VecDeque<BlockEvent>,
    responses: HashMap<u64, oneshot::Sender<Value>>,
    /// Simulation only: which block parked awaiting each token, so the
    /// response can make it runnable during the responder's turn.
    callers: HashMap<u64, String>,
    shutdown: ShutdownFlags,
    interface: Option<Value>,
    last_error: Option<String>,
}

/// The single shared state record for one block instance.
pub struct BlockCell {
    /// The block's local name within its assembly.
    pub name: String,
    /// The block's runtime-assigned identity.
    pub id: BlockId,
    /// Host-only admission identity; transcript IDs may repeat across runtimes.
    pub(crate) admission_id: BlockId,
    /// Failure policy from the assembly definition.
    pub failure: FailurePolicy,
    /// Cancelled on immediate shutdown: fails the block's parked reads.
    pub cancel: CancelToken,

    state: Mutex<CellState>,
    /// Notified on every state/queue/shutdown change a waiter might watch.
    pub(crate) gate: Gate,
    next_token: AtomicU64,
    started_at: Instant,
    /// Deterministic simulation (spec 12): the schedule this cell's
    /// wakes report to, under the cell's stable key. `None` outside
    /// simulation.
    sim: Option<(String, std::sync::Arc<crate::turnstile::Turnstile>)>,
}

impl BlockCell {
    /// Create a cell in `Created` state.
    pub fn new(name: impl Into<String>, failure: FailurePolicy) -> Self {
        Self {
            name: name.into(),
            id: BlockId::new(),
            admission_id: BlockId::new(),
            failure,
            cancel: CancelToken::new(),
            state: Mutex::new(CellState {
                state: BlockState::Created,
                queue: VecDeque::new(),
                responses: HashMap::new(),
                shutdown: ShutdownFlags {
                    requested: false,
                    mode: None,
                    complete: false,
                    exit_code: None,
                },
                interface: None,
                last_error: None,
                callers: HashMap::new(),
            }),
            gate: Gate::new(),
            next_token: AtomicU64::new(0),
            started_at: Instant::now(),
            sim: None,
        }
    }

    /// Enroll this cell in a deterministic simulation before it is
    /// shared: its wakes then report to the turnstile under `key`.
    pub(crate) fn attach_simulation(
        &mut self,
        key: &str,
        turnstile: std::sync::Arc<crate::turnstile::Turnstile>,
    ) {
        self.sim = Some((key.to_string(), turnstile));
    }

    /// Whether this cell runs under the deterministic scheduler.
    pub fn simulated(&self) -> bool {
        self.sim.is_some()
    }

    /// The cell's schedule enrollment, when simulated.
    pub(crate) fn sim(&self) -> Option<&(String, std::sync::Arc<crate::turnstile::Turnstile>)> {
        self.sim.as_ref()
    }

    /// A cell whose id derives from its assembly-scoped key, so
    /// identity is stable across runs (see [`BlockId::named`]).
    pub fn keyed(name: impl Into<String>, failure: FailurePolicy, key: &str) -> Self {
        Self {
            name: name.into(),
            id: BlockId::named(key),
            admission_id: BlockId::new(),
            failure,
            cancel: CancelToken::new(),
            state: Mutex::new(CellState {
                state: BlockState::Created,
                queue: VecDeque::new(),
                responses: HashMap::new(),
                shutdown: ShutdownFlags {
                    requested: false,
                    mode: None,
                    complete: false,
                    exit_code: None,
                },
                interface: None,
                last_error: None,
                callers: HashMap::new(),
            }),
            gate: Gate::new(),
            next_token: AtomicU64::new(0),
            started_at: Instant::now(),
            sim: None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CellState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    // === Lifecycle ===

    /// Current lifecycle state.
    pub fn state(&self) -> BlockState {
        self.lock().state
    }

    /// Transition state and wake watchers.
    pub fn set_state(&self, state: BlockState) {
        let orphaned = {
            let mut cell = self.lock();
            cell.state = state;
            if state.is_terminal() {
                // No response will ever come: fail in-flight callers by
                // dropping their senders.
                cell.responses.clear();
                std::mem::take(&mut cell.callers)
            } else {
                HashMap::new()
            }
        };
        self.gate.notify();
        // Simulated callers parked on those responses see the dropped
        // sender only if the schedule runs them again.
        if let Some((_, turnstile)) = &self.sim {
            for caller in orphaned.into_values() {
                turnstile.wake_response(&caller);
            }
        }
    }

    /// Attempt the Created -> Starting transition. Returns true if this
    /// caller won the race and should spawn the driver.
    pub fn try_begin_start(&self) -> bool {
        let mut cell = self.lock();
        if cell.state == BlockState::Created {
            cell.state = BlockState::Starting;
            true
        } else {
            false
        }
    }

    /// Record a failure message for diagnostics.
    pub fn record_error(&self, message: impl Into<String>) {
        self.lock().last_error = Some(message.into());
    }

    /// Last recorded failure, if any.
    pub fn last_error(&self) -> Option<String> {
        self.lock().last_error.clone()
    }

    /// Nanoseconds since the cell was created (the block's monotonic clock).
    pub fn monotonic_nanos(&self) -> i64 {
        self.started_at.elapsed().as_nanos() as i64
    }

    // === Server protocol: caller side ===

    /// Queue a request for this block and return the response receiver.
    ///
    /// The caller awaits the receiver; a dropped receiver (block reached a
    /// terminal state) means the store is unavailable.
    pub fn enqueue(&self, op: &'static str, path: Path, data: Value) -> oneshot::Receiver<Value> {
        self.enqueue_inner(op, path, data).1
    }

    fn enqueue_inner(
        &self,
        op: &'static str,
        path: Path,
        data: Value,
    ) -> (u64, oneshot::Receiver<Value>) {
        let token = self.next_token.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut cell = self.lock();
            if cell.state.is_terminal() || cell.shutdown.requested {
                return (token, rx);
            }
            cell.responses.insert(token, tx);
            if self.sim.is_some() {
                // Remember who parks on this token, so the response can
                // make them runnable during the responder's turn.
                if let Some(caller) = crate::turnstile::current_block() {
                    cell.callers.insert(token, caller);
                }
            }
            cell.queue.push_back(BlockEvent::Request(ServerRequest {
                op,
                path,
                data,
                token,
            }));
        }
        self.gate.notify();
        // A mailbox event landed: under simulation the owner, if idle,
        // is now runnable (delivered during the caller's turn, so the
        // runnable set stays a function of the run).
        if let Some((key, turnstile)) = &self.sim {
            turnstile.wake_mailbox(key);
        }
        (token, rx)
    }

    pub(crate) fn enqueue_owned(
        self: &std::sync::Arc<Self>,
        op: &'static str,
        path: Path,
        data: Value,
        charge: crate::admission::CallCharge,
    ) -> PendingCall {
        let (token, receiver) = self.enqueue_inner(op, path, data);
        PendingCall {
            cell: self.clone(),
            token,
            receiver,
            _charge: charge,
        }
    }

    /// Outstanding response identities and queued events, for overload diagnostics.
    pub fn pending_counts(&self) -> (usize, usize) {
        let cell = self.lock();
        (cell.responses.len(), cell.queue.len())
    }

    /// Deliver a signal to the block's mailbox.
    pub fn deliver_signal(&self, name: impl Into<String>, data: Value) {
        self.lock().queue.push_back(BlockEvent::Signal {
            name: name.into(),
            data,
        });
        self.gate.notify();
        if let Some((key, turnstile)) = &self.sim {
            turnstile.wake_mailbox(key);
        }
    }

    /// Deliver a timer expiry to the block's mailbox.
    pub fn deliver_timer(&self, tag: Value) {
        self.lock().queue.push_back(BlockEvent::Timer { tag });
        self.gate.notify();
        if let Some((key, turnstile)) = &self.sim {
            turnstile.wake_mailbox(key);
        }
    }

    // === Server protocol: block side ===

    /// Take the next mailbox event, parking until one arrives or shutdown
    /// is requested (which yields `None`, the spec's null-unblock).
    pub async fn next_event(&self) -> Result<Option<BlockEvent>, Error> {
        // First read of the mailbox marks the block Running (the spec's
        // Starting -> Running transition: "begins reading").
        {
            let mut cell = self.lock();
            if cell.state == BlockState::Starting {
                cell.state = BlockState::Running;
            }
        }
        // Under simulation the park goes through the turnstile: the
        // empty-check runs while holding the turn (no producer can be
        // mid-enqueue), and the wake that refills the queue marks this
        // block runnable during the producer's turn.
        if let Some((key, turnstile)) = &self.sim {
            loop {
                if self.cancel.is_cancelled() {
                    return Err(structfs_core_store::Error::cancelled(
                        "block shutdown (immediate)",
                    ));
                }
                {
                    let mut cell = self.lock();
                    if let Some(event) = cell.queue.pop_front() {
                        return Ok(Some(event));
                    }
                    if cell.shutdown.requested {
                        return Ok(None);
                    }
                }
                // Idle on the mailbox: quiescence, not a dependency.
                turnstile.park(key, crate::turnstile::ParkKind::Mailbox);
                turnstile.wait_turn(key).await;
            }
        }
        self.gate
            .wait_until_cancellable(&self.cancel, || {
                let mut cell = self.lock();
                if let Some(event) = cell.queue.pop_front() {
                    return Some(Some(event));
                }
                if cell.shutdown.requested {
                    return Some(None);
                }
                None
            })
            .await
            .map_err(|c| c.into_error("block shutdown (immediate)"))
    }

    /// Drain all pending mailbox events without blocking.
    pub fn pending_events(&self) -> Vec<BlockEvent> {
        let mut cell = self.lock();
        cell.queue.drain(..).collect()
    }

    /// Fulfill a response for a correlation token. Unknown tokens are
    /// ignored (the caller may have timed out and gone away).
    pub fn respond(&self, token: u64, response: Value) {
        let (sender, caller) = {
            let mut cell = self.lock();
            (cell.responses.remove(&token), cell.callers.remove(&token))
        };
        if let Some(sender) = sender {
            let _ = sender.send(response);
        }
        // The parked caller's answer exists: runnable, during this
        // (the responder's) turn.
        if let (Some((_, turnstile)), Some(caller)) = (&self.sim, caller) {
            turnstile.wake_response(&caller);
        }
    }

    /// Drop every in-flight response sender, failing callers awaiting
    /// this block, and wake them so their calls return an error. Used
    /// when the schedule wedges: a caller stuck in a call `.await` is
    /// freed by its response never coming, and this makes "never" now.
    pub(crate) fn fail_in_flight(&self) {
        let callers = {
            let mut cell = self.lock();
            cell.responses.clear();
            std::mem::take(&mut cell.callers)
        };
        if let Some((_, turnstile)) = &self.sim {
            for caller in callers.into_values() {
                turnstile.wake_response(&caller);
            }
        }
    }

    // === Shutdown ===

    /// Request shutdown; wakes parked request reads.
    pub fn request_shutdown(&self, mode: ShutdownMode) {
        {
            let mut cell = self.lock();
            cell.shutdown.requested = true;
            cell.shutdown.mode = Some(mode);
            if !cell.state.is_terminal() && cell.state != BlockState::Created {
                cell.state = BlockState::Stopping;
            } else if cell.state == BlockState::Created {
                // Never started: nothing to drain.
                cell.state = BlockState::Stopped;
            }
        }
        self.gate.notify();
        // A shutdown request is a mailbox wake: a simulated block idle
        // on its mailbox must run to observe the null-unblock. A
        // call-parked block is freed instead by its response failing
        // (see `fail_in_flight`).
        if let Some((key, turnstile)) = &self.sim {
            turnstile.wake_mailbox(key);
        }
        if mode == ShutdownMode::Immediate {
            self.cancel.cancel();
        }
    }

    /// Whether shutdown has been requested.
    pub fn shutdown_requested(&self) -> bool {
        self.lock().shutdown.requested
    }

    /// The shutdown mode, if requested.
    pub fn shutdown_mode(&self) -> Option<ShutdownMode> {
        self.lock().shutdown.mode
    }

    /// Block signals its shutdown is complete, with an exit code.
    pub fn mark_shutdown_complete(&self, code: i64) {
        {
            let mut cell = self.lock();
            cell.shutdown.complete = true;
            cell.shutdown.exit_code = Some(code);
        }
        self.gate.notify();
    }

    /// Whether the block signalled shutdown completion.
    pub fn shutdown_complete(&self) -> bool {
        self.lock().shutdown.complete
    }

    /// The block's exit code: what it declared via `shutdown/complete`,
    /// defaulting to 0 for a clean stop and 1 for failure.
    pub fn exit_code(&self) -> i64 {
        let cell = self.lock();
        match cell.shutdown.exit_code {
            Some(code) => code,
            None if cell.state == BlockState::Failed => 1,
            None => 0,
        }
    }

    /// Terminal-status envelope: `{name, state, code}`.
    pub fn status_value(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert("name".to_string(), Value::String(self.name.clone()));
        map.insert("state".to_string(), Value::from(self.state().as_str()));
        map.insert("code".to_string(), Value::Integer(self.exit_code()));
        Value::Map(map)
    }

    /// Park until this cell reaches a terminal state.
    pub async fn wait_terminal(&self) {
        self.gate
            .wait_until(|| self.lock().state.is_terminal().then_some(()))
            .await
    }

    // === Interface declaration ===

    /// Store the block's runtime interface declaration
    /// (`/iso/self/interface`).
    pub fn set_interface(&self, interface: Value) {
        self.lock().interface = Some(interface);
    }

    /// The declared interface, if any.
    pub fn interface(&self) -> Option<Value> {
        self.lock().interface.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use structfs_core_store::path;

    #[tokio::test]
    async fn enqueue_and_serve_round_trip() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        cell.set_state(BlockState::Starting);

        let rx = cell.enqueue("read", path!("users/1"), Value::Null);

        let event = cell.next_event().await.unwrap().unwrap();
        let request = match event {
            BlockEvent::Request(request) => request,
            other => panic!("expected request, got {other:?}"),
        };
        assert_eq!(request.op, "read");
        assert_eq!(request.path, path!("users/1"));
        assert_eq!(cell.state(), BlockState::Running);

        cell.respond(request.token, Value::from("response"));
        assert_eq!(rx.await.unwrap(), Value::from("response"));
    }

    #[tokio::test]
    async fn next_event_parks_until_enqueue() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        let server = {
            let cell = cell.clone();
            tokio::spawn(async move { cell.next_event().await })
        };
        tokio::task::yield_now().await;
        let _rx = cell.enqueue("write", path!("k"), Value::from(1i64));
        assert!(server.await.unwrap().unwrap().is_some());
    }

    #[tokio::test]
    async fn mailbox_interleaves_signals_and_timers() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        cell.set_state(BlockState::Running);
        let _rx = cell.enqueue("read", path!("a"), Value::Null);
        cell.deliver_signal("usr1", Value::from(1i64));
        cell.deliver_timer(Value::from("flush"));

        assert!(matches!(
            cell.next_event().await.unwrap().unwrap(),
            BlockEvent::Request(_)
        ));
        assert!(matches!(
            cell.next_event().await.unwrap().unwrap(),
            BlockEvent::Signal { ref name, .. } if name == "usr1"
        ));
        assert!(matches!(
            cell.next_event().await.unwrap().unwrap(),
            BlockEvent::Timer { ref tag } if *tag == Value::from("flush")
        ));
    }

    #[tokio::test]
    async fn signal_wakes_parked_mailbox_read() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        let server = {
            let cell = cell.clone();
            tokio::spawn(async move { cell.next_event().await })
        };
        tokio::task::yield_now().await;
        cell.deliver_signal("wake", Value::Null);
        assert!(matches!(
            server.await.unwrap().unwrap(),
            Some(BlockEvent::Signal { .. })
        ));
    }

    #[tokio::test]
    async fn graceful_shutdown_unblocks_with_none() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        let server = {
            let cell = cell.clone();
            tokio::spawn(async move { cell.next_event().await })
        };
        tokio::task::yield_now().await;
        cell.request_shutdown(ShutdownMode::Graceful);
        assert!(server.await.unwrap().unwrap().is_none());
        assert_eq!(cell.shutdown_mode(), Some(ShutdownMode::Graceful));
    }

    #[tokio::test]
    async fn immediate_shutdown_cancels_parked_read() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        cell.set_state(BlockState::Running);
        let server = {
            let cell = cell.clone();
            tokio::spawn(async move { cell.next_event().await })
        };
        tokio::task::yield_now().await;
        cell.request_shutdown(ShutdownMode::Immediate);
        let result = server.await.unwrap();
        // Parked read fails on immediate shutdown (cancellation).
        assert!(result.is_err() || result.unwrap().is_none());
    }

    #[test]
    fn exit_codes() {
        let cell = BlockCell::new("test", FailurePolicy::FailFast);
        assert_eq!(cell.exit_code(), 0);
        cell.mark_shutdown_complete(3);
        cell.set_state(BlockState::Stopped);
        assert_eq!(cell.exit_code(), 3);

        let failed = BlockCell::new("bad", FailurePolicy::FailFast);
        failed.set_state(BlockState::Failed);
        assert_eq!(failed.exit_code(), 1);

        match failed.status_value() {
            Value::Map(map) => {
                assert_eq!(map.get("state"), Some(&Value::from("failed")));
                assert_eq!(map.get("code"), Some(&Value::Integer(1)));
            }
            _ => panic!("expected map"),
        }
    }

    #[tokio::test]
    async fn terminal_state_fails_inflight_callers() {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        let rx = cell.enqueue("read", path!("x"), Value::Null);
        cell.set_state(BlockState::Failed);
        assert!(rx.await.is_err());
    }

    #[test]
    fn pending_drains_queue() {
        let cell = BlockCell::new("test", FailurePolicy::FailFast);
        let _r1 = cell.enqueue("read", path!("a"), Value::Null);
        let _r2 = cell.enqueue("read", path!("b"), Value::Null);
        assert_eq!(cell.pending_events().len(), 2);
        assert_eq!(cell.pending_events().len(), 0);
    }

    #[test]
    fn request_envelope_shape() {
        let request = ServerRequest {
            op: "write",
            path: path!("users/1"),
            data: Value::from(5i64),
            token: 9,
        };
        match request.to_value() {
            Value::Map(map) => {
                assert_eq!(map.get("op"), Some(&Value::from("write")));
                assert_eq!(map.get("path"), Some(&Value::from("users/1")));
                assert_eq!(map.get("data"), Some(&Value::Integer(5)));
                assert_eq!(
                    map.get("respond_to"),
                    Some(&Value::from("iso/server/responses/9"))
                );
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn shutdown_of_created_block_stops_directly() {
        let cell = BlockCell::new("idle", FailurePolicy::Isolate);
        cell.request_shutdown(ShutdownMode::Graceful);
        assert_eq!(cell.state(), BlockState::Stopped);
    }
}
