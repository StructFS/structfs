//! The in-memory reference state provider: commits, handles and retention.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
};

use structfs_core_store::{Error, Path, Value};
use structfs_handles::Gate;
use structfs_service::{CallContext, OwnerHandle, Registration, ResourceKind, Service};
use tokio::time::Instant;

use crate::faults::Faults;
use crate::limits::{invalid, limit, size, validate, validate_shape, StateLimits};
use crate::paging::{flatten, SizedNode};
use crate::{Change, Command, Descriptor, Fault, Mutation, ReadLimits, Request, Token};

pub(crate) struct History {
    pub(crate) change: Change,
    pub(crate) bytes: usize,
}

/// Who submitted a command: the view it arrived through and, when the call
/// was bound to an owner (`Client::owned_by`), that owner.
///
/// Handles and faults answer only to the principal that created them. Two
/// views granted the same subtree are still different principals, so one
/// tenant cannot read, release or erase another tenant's handles by guessing
/// ids — and ids are random besides.
///
/// **Isolation is per view, by design.** Every client that reaches the same
/// `View` instance without an owner binding is the *same* principal: the view
/// is the grant, and whoever holds the grant holds its handles. A host that
/// needs isolation between blocks or tenants must call `State::view` once per
/// block or tenant (or bind each caller's client with `owned_by`), never share
/// one view among them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Principal {
    pub(crate) view: u64,
    pub(crate) owner: Option<u64>,
}

/// A live handle: either a pinned read (snapshot/watch) or a committed batch
/// receipt. Every one of these holds an owner registration and charges the
/// retention budget.
pub(crate) struct Handle {
    pub(crate) principal: Principal,
    pub(crate) prefix: Path,
    pub(crate) descriptor: Descriptor,
    pub(crate) nodes: Vec<SizedNode>,
    pub(crate) snapshot_bytes: usize,
    pub(crate) limits: ReadLimits,
    pub(crate) expires: Instant,
    registration: Registration,
}
impl Handle {
    pub(crate) fn is_released(&self) -> bool {
        self.registration.cancellation().is_cancelled()
    }
}

pub(crate) struct Inner {
    pub(crate) root: Option<Value>,
    pub(crate) revision: u64,
    pub(crate) history: VecDeque<History>,
    history_bytes: usize,
    pub(crate) handles: BTreeMap<String, Handle>,
    /// Commands that never became a handle; see [`crate::faults`] for the
    /// per-principal and global bounds.
    faults: Faults,
    snapshot_bytes: usize,
    pub(crate) closed: bool,
}

/// In-memory durability only. Create a restricted `view` for each granted subtree.
/// Command payload paths are relative to that view, never global router paths.
pub struct State {
    pub(crate) inner: Mutex<Inner>,
    pub(crate) epoch: String,
    pub(crate) limits: StateLimits,
    pub(crate) owner: OwnerHandle,
    pub(crate) gate: Gate,
    next_view: AtomicU64,
    lifetime: Mutex<Option<Registration>>,
}

impl State {
    /// Build the state without registering its lifetime. Private: a `State`
    /// that has not registered its teardown and retained-bytes reservation
    /// with its owner is not a correct `State`, so [`State::shared`] is the
    /// only public constructor.
    fn new(
        owner: &OwnerHandle,
        initial: Option<Value>,
        limits: StateLimits,
    ) -> Result<Self, Error> {
        if limits.history_records == 0
            || limits.change_bytes == 0
            || limits.change_bytes > limits.history_bytes
            || limits.page_bytes
                < limits
                    .change_bytes
                    .saturating_add(crate::limits::PAGE_OVERHEAD)
            || limits.page_items == 0
            || limits.depth > 48
            || limits.handle_age.is_zero()
            || Instant::now().checked_add(limits.handle_age).is_none()
        {
            return Err(Error::invalid_argument("incompatible state limits"));
        }
        if let Some(v) = &initial {
            validate(v, &limits).map_err(|e| Error::resource_limit(e.to_string()))?;
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                root: initial,
                revision: 0,
                history: VecDeque::new(),
                history_bytes: 0,
                handles: BTreeMap::new(),
                faults: Faults::default(),
                snapshot_bytes: 0,
                closed: false,
            }),
            epoch: uuid::Uuid::new_v4().to_string(),
            limits,
            owner: owner.clone(),
            gate: Gate::new(),
            next_view: AtomicU64::new(0),
            lifetime: Mutex::new(None),
        })
    }

    /// Create a state owned by `owner`, returned shared.
    ///
    /// This is the only constructor, and the one exception to the
    /// "constructors return `Self`" convention: a `State` registers its own
    /// teardown and a retained-bytes reservation with `owner`, and that
    /// cleanup must reach the state through a weak `Arc` reference. A bare
    /// `State` would skip both.
    pub fn shared(
        owner: &OwnerHandle,
        initial: Option<Value>,
        limits: StateLimits,
    ) -> Result<Arc<Self>, Error> {
        let state = Arc::new(Self::new(owner, initial, limits)?);
        let weak = Arc::downgrade(&state);
        let reserved = state
            .limits
            .state_bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(state.limits.history_bytes))
            .and_then(|n| n.checked_add(state.limits.snapshot_bytes))
            .ok_or_else(|| Error::resource_limit("state reservation overflow"))?;
        let registration =
            owner.register(ResourceKind::Retained, reserved, move || async move {
                if let Some(s) = weak.upgrade() {
                    let mut inner = s.inner.lock().unwrap_or_else(|e| e.into_inner());
                    inner.closed = true;
                    inner.root = None;
                    inner.history.clear();
                    inner.history_bytes = 0;
                    inner.handles.clear();
                    inner.faults.clear();
                    inner.snapshot_bytes = 0;
                    drop(inner);
                    s.gate.notify();
                }
                Ok(())
            })?;
        *state.lifetime.lock().unwrap_or_else(|e| e.into_inner()) = Some(registration);
        Ok(state)
    }

    /// A restricted service over `base`. Every call creates a distinct
    /// principal: handles opened through one view are invisible to another,
    /// even one granted the same subtree.
    pub fn view(self: &Arc<Self>, base: Path, writable: bool) -> Arc<dyn Service> {
        let id = self.next_view.fetch_add(1, Ordering::Relaxed);
        Arc::new(crate::view::View::new(self.clone(), id, base, writable))
    }

    pub fn token(&self) -> Token {
        self.token_at(
            self.inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .revision,
        )
    }
    pub(crate) fn token_at(&self, revision: u64) -> Token {
        Token {
            epoch: self.epoch.clone(),
            revision,
        }
    }

    /// Drop a handle's table entry. Only the handle's own registration
    /// cleanup and [`State::release`] call this.
    fn remove_handle(inner: &mut Inner, id: &str) {
        if let Some(h) = inner.handles.remove(id) {
            inner.snapshot_bytes -= h.snapshot_bytes;
        }
    }

    /// Release `id` on behalf of `principal`: a handle or fault it owns is
    /// dropped. An id it does not own behaves exactly like an unknown id — a
    /// no-op — so, as with reads, probing ids reveals nothing. The one refusal
    /// is releasing the caller's *own* committed-batch receipt through a
    /// read-only view.
    pub(crate) fn release(
        inner: &mut Inner,
        id: &str,
        principal: &Principal,
        writable: bool,
    ) -> Result<(), Error> {
        if let Some(handle) = Self::handle(inner, id, principal) {
            if handle.descriptor.committed && !writable {
                return Err(Error::permission_denied(
                    "releasing a write receipt requires a writable view",
                ));
            }
            Self::remove_handle(inner, id);
        } else {
            inner.faults.release(id, principal);
        }
        Ok(())
    }

    pub(crate) fn reap(inner: &mut Inner) {
        let now = Instant::now();
        let ids: Vec<_> = inner
            .handles
            .iter()
            .filter(|(_, h)| h.expires <= now || h.is_released())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            Self::remove_handle(inner, &id);
        }
        inner.faults.expire(now);
    }

    /// The live handle `id`, if `principal` owns it.
    pub(crate) fn handle<'a>(
        inner: &'a Inner,
        id: &str,
        principal: &Principal,
    ) -> Option<&'a Handle> {
        inner.handles.get(id).filter(|h| &h.principal == principal)
    }

    /// The fault recorded for a rejected command, if `principal` owns it and
    /// it has been neither released nor aged out.
    pub(crate) fn fault(inner: &Inner, id: &str, principal: &Principal) -> Option<Fault> {
        inner.faults.get(id, principal).cloned()
    }

    pub(crate) fn check_token(&self, inner: &Inner, after: &Token) -> Result<(), Fault> {
        if after.epoch != self.epoch {
            return Err(Fault::EpochMismatch {
                current: self.token_at(inner.revision),
            });
        }
        if after.revision > inner.revision {
            return Err(invalid("future revision"));
        }
        let earliest = inner
            .history
            .front()
            .map_or(inner.revision, |h| h.change.token.revision - 1);
        if after.revision < earliest {
            return Err(Fault::CursorExpired {
                earliest: self.token_at(earliest),
            });
        }
        Ok(())
    }

    fn check_read_limits(&self, l: &ReadLimits) -> Result<(), Fault> {
        if l.page_items == 0
            || l.page_items > self.limits.page_items
            || l.page_bytes > self.limits.page_bytes
            || l.page_bytes
                < self
                    .limits
                    .change_bytes
                    .saturating_add(crate::limits::PAGE_OVERHEAD)
        {
            return Err(limit("incompatible page limits"));
        }
        Ok(())
    }

    /// A fresh id: a random `u64`, decimal on the wire. Principal checks are
    /// the access control; unguessable ids are defence in depth.
    fn mint_id(inner: &Inner) -> String {
        loop {
            let id = uuid::Uuid::new_v4().as_u64_pair().0.to_string();
            if !inner.handles.contains_key(&id) && !inner.faults.contains(&id) {
                return id;
            }
        }
    }

    /// Record a rejected command without minting a handle or a registration.
    /// Bounded by `principal`'s own budget (`limits.handles`) and by the
    /// global `limits.max_faults`.
    fn record_fault(&self, inner: &mut Inner, principal: &Principal, fault: Fault) -> String {
        let id = Self::mint_id(inner);
        let expires = Instant::now()
            .checked_add(self.limits.handle_age)
            .unwrap_or_else(Instant::now);
        inner.faults.insert(
            id.clone(),
            principal,
            fault,
            expires,
            self.limits.handles,
            self.limits.max_faults,
        );
        id
    }

    /// Accept a command and return the handle id to read its reply from.
    ///
    /// The expensive half — canonical-encoding every mutation payload — runs
    /// before the state lock is taken. Under the lock the proposed tree is
    /// encoded exactly once, no matter how many mutations the batch carries.
    pub(crate) fn command(
        self: &Arc<Self>,
        base: &Path,
        principal: &Principal,
        writable: bool,
        context: &CallContext,
        request: Request,
    ) -> Result<String, Error> {
        context.ensure_active()?;
        self.owner.ensure_open()?;

        // Lock-free pre-flight. A malformed command is rejected here, before
        // it can touch the single state mutex at all.
        let precheck = self.precheck(&request, writable);

        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::reap(&mut inner);
        if inner.closed {
            return Err(Error::cancelled("state closed"));
        }
        if let Err(fault) = precheck {
            return Ok(self.record_fault(&mut inner, principal, fault));
        }

        let current = self.token_at(inner.revision);
        let prepared = match request.command {
            Command::Batch {
                expected,
                mutations,
            } => self.prepare_batch(&inner, base, &current, expected, mutations),
            Command::Snapshot { prefix, limits } => self.prepare_read(
                &inner,
                base,
                ReadSpec {
                    relative: prefix,
                    limits,
                    snapshot: true,
                    after: None,
                },
                &current,
            ),
            Command::Observe { prefix, limits } => self.prepare_read(
                &inner,
                base,
                ReadSpec {
                    relative: prefix,
                    limits,
                    snapshot: true,
                    after: Some(current.clone()),
                },
                &current,
            ),
            Command::Watch {
                prefix,
                after,
                limits,
            } => self.prepare_read(
                &inner,
                base,
                ReadSpec {
                    relative: prefix,
                    limits,
                    snapshot: false,
                    after: Some(after),
                },
                &current,
            ),
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(fault) => return Ok(self.record_fault(&mut inner, principal, fault)),
        };

        // Per principal first, like faults, so one view or owner holding its
        // whole budget cannot starve another; then the global ceiling.
        let held = inner
            .handles
            .values()
            .filter(|h| &h.principal == principal)
            .count();
        if held >= self.limits.handles {
            return Err(Error::overloaded("state handle limit for this principal"));
        }
        if inner.handles.len() >= self.limits.max_handles {
            return Err(Error::overloaded("state handle limit"));
        }
        if prepared.bytes
            > self
                .limits
                .total_snapshot_bytes
                .saturating_sub(inner.snapshot_bytes)
        {
            return Err(Error::overloaded("retained handle bytes"));
        }
        let expires = Instant::now()
            .checked_add(self.limits.handle_age)
            .ok_or_else(|| Error::resource_limit("handle age overflow"))?;

        let id = Self::mint_id(&inner);
        let weak: Weak<Self> = Arc::downgrade(self);
        let cleanup_id = id.clone();
        let owner = context.owner().unwrap_or(&self.owner);
        let registration =
            owner.register(ResourceKind::Retained, prepared.bytes, move || async move {
                if let Some(state) = weak.upgrade() {
                    Self::remove_handle(
                        &mut state.inner.lock().unwrap_or_else(|e| e.into_inner()),
                        &cleanup_id,
                    );
                    state.gate.notify();
                }
                Ok(())
            })?;
        context.ensure_active()?;
        owner.ensure_open()?;
        self.owner.ensure_open()?;

        if let Some((root, history)) = prepared.commit {
            inner.root = root;
            inner.revision = history.change.token.revision;
            inner.history_bytes += history.bytes;
            inner.history.push_back(history);
            while inner.history.len() > self.limits.history_records
                || inner.history_bytes > self.limits.history_bytes
            {
                inner.history_bytes -= inner.history.pop_front().unwrap().bytes;
            }
        }
        inner.snapshot_bytes += prepared.bytes;
        inner.handles.insert(
            id.clone(),
            Handle {
                principal: principal.clone(),
                prefix: prepared.prefix,
                descriptor: prepared.descriptor,
                nodes: prepared.nodes,
                snapshot_bytes: prepared.bytes,
                limits: prepared.limits,
                expires,
                registration,
            },
        );
        drop(inner);
        self.gate.notify();
        Ok(id)
    }

    /// Everything that can be decided without the state lock: protocol
    /// version, write permission, batch shape, and the shape and canonical
    /// size of each mutation payload.
    fn precheck(&self, request: &Request, writable: bool) -> Result<(), Fault> {
        if request.version != 1 {
            return Err(invalid("unsupported state protocol version"));
        }
        let Command::Batch { mutations, .. } = &request.command else {
            return Ok(());
        };
        if !writable {
            return Err(invalid("view is read only"));
        }
        if mutations.is_empty() || mutations.len() > self.limits.mutations {
            return Err(limit("batch count"));
        }
        for mutation in mutations {
            if let Mutation::Set { value, .. } = mutation {
                validate_shape(value, &self.limits)?;
                if size(value)? > self.limits.state_bytes {
                    return Err(limit("state bytes"));
                }
            }
        }
        Ok(())
    }

    fn prepare_batch(
        &self,
        inner: &Inner,
        base: &Path,
        current: &Token,
        expected: Option<Token>,
        mutations: Vec<Mutation>,
    ) -> Result<Prepared, Fault> {
        if let Some(expected) = expected {
            if expected.epoch != self.epoch {
                return Err(Fault::EpochMismatch {
                    current: current.clone(),
                });
            }
            if &expected != current {
                return Err(Fault::Conflict {
                    current: current.clone(),
                });
            }
        }
        let revision = inner
            .revision
            .checked_add(1)
            .ok_or_else(|| limit("revision exhausted"))?;
        let mut root = inner.root.clone();
        let mut touched = BTreeSet::new();
        for mutation in mutations {
            let relative = parse_relative(mutation.path())?;
            let target = base.join(&relative);
            if target.len() > self.limits.depth {
                return Err(limit("mutation path depth"));
            }
            // Conservative parent invalidation also covers array index shifts.
            touched.insert(target.slice(0, target.len().saturating_sub(1)).to_string());
            match mutation {
                Mutation::Set { value, .. } => {
                    root.get_or_insert_with(|| Value::Map(BTreeMap::new()))
                        .set(&target, value)
                        .map_err(|_| invalid("set traversal"))?;
                }
                Mutation::Delete { .. } => {
                    if target.is_empty() {
                        root = None;
                    } else if let Some(v) = &mut root {
                        v.remove(&target).map_err(|_| invalid("delete traversal"))?;
                    }
                }
            }
        }
        // Once, on the proposed tree — not once per mutation. Each mutation's
        // own payload was already bounded before the lock was taken, so the
        // intermediate trees are bounded too.
        if let Some(v) = &root {
            validate(v, &self.limits)?;
        }
        let change = Change {
            token: self.token_at(revision),
            paths: touched.into_iter().collect(),
        };
        let bytes = size(&change)?;
        if bytes > self.limits.change_bytes {
            return Err(limit("atomic change record"));
        }
        Ok(Prepared {
            descriptor: Descriptor {
                token: change.token.clone(),
                snapshot: false,
                watch: false,
                committed: true,
            },
            prefix: base.clone(),
            nodes: vec![],
            bytes: size(&base.to_string())?.saturating_add(crate::limits::PAGE_OVERHEAD),
            limits: ReadLimits::default(),
            commit: Some((root, History { change, bytes })),
        })
    }

    fn prepare_read(
        &self,
        inner: &Inner,
        base: &Path,
        spec: ReadSpec,
        current: &Token,
    ) -> Result<Prepared, Fault> {
        let ReadSpec {
            relative,
            limits,
            snapshot,
            after,
        } = spec;
        self.check_read_limits(&limits)?;
        if let Some(t) = &after {
            self.check_token(inner, t)?;
        }
        let prefix = base.join(&parse_relative(&relative)?);
        let nodes = if snapshot {
            flatten(
                inner.root.as_ref().and_then(|v| v.get(&prefix)),
                &self.limits,
            )?
        } else {
            vec![]
        };
        let bytes = nodes
            .iter()
            .fold(0usize, |n, node| n.saturating_add(node.bytes))
            .saturating_add(size(&prefix.to_string())?)
            .saturating_add(crate::limits::PAGE_OVERHEAD);
        if bytes > self.limits.snapshot_bytes
            || bytes
                > self
                    .limits
                    .total_snapshot_bytes
                    .saturating_sub(inner.snapshot_bytes)
        {
            return Err(limit("snapshot retention"));
        }
        for node in &nodes {
            if node.bytes.saturating_add(crate::limits::PAGE_OVERHEAD) > limits.page_bytes {
                return Err(limit("snapshot node cannot fit page"));
            }
        }
        Ok(Prepared {
            descriptor: Descriptor {
                token: after.clone().unwrap_or_else(|| current.clone()),
                snapshot,
                watch: after.is_some(),
                committed: false,
            },
            prefix,
            nodes,
            bytes,
            limits,
            commit: None,
        })
    }
}

fn parse_relative(s: &str) -> Result<Path, Fault> {
    Path::parse(s).map_err(|_| invalid("invalid relative path"))
}

/// A snapshot, observe or watch command, as [`State::prepare_read`] takes it.
struct ReadSpec {
    /// Prefix relative to the view's base.
    relative: String,
    limits: ReadLimits,
    /// Pin the subtree's nodes (snapshot/observe) or not (watch).
    snapshot: bool,
    /// The change cursor, for observe and watch.
    after: Option<Token>,
}

struct Prepared {
    descriptor: Descriptor,
    prefix: Path,
    nodes: Vec<SizedNode>,
    bytes: usize,
    limits: ReadLimits,
    commit: Option<(Option<Value>, History)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::counter;

    fn set(path: &str, value: Value) -> Mutation {
        Mutation::Set {
            path: path.into(),
            value,
        }
    }
    fn me() -> Principal {
        Principal {
            view: 0,
            owner: None,
        }
    }
    fn other() -> Principal {
        Principal {
            view: 1,
            owner: None,
        }
    }

    async fn state_with(
        initial: Value,
        limits: StateLimits,
    ) -> (structfs_service::Owner, Arc<State>) {
        let supervisor = structfs_service::CleanupSupervisor::new(1).unwrap();
        let owner = supervisor
            .owner(structfs_service::OwnerLimits::default().with_resources(4096))
            .unwrap();
        let state = State::shared(&owner.handle(), Some(initial), limits).unwrap();
        // Keep the supervisor alive for the owner's lifetime.
        std::mem::forget(supervisor);
        (owner, state)
    }
    async fn state(limits: StateLimits) -> (structfs_service::Owner, Arc<State>) {
        state_with(Value::Map(BTreeMap::new()), limits).await
    }

    #[tokio::test]
    async fn revision_overflow_fails_before_commit() {
        let (_owner, state) = state_with(Value::Null, StateLimits::default()).await;
        state.inner.lock().unwrap().revision = u64::MAX;
        let id = state
            .command(
                &Path::parse("").unwrap(),
                &me(),
                true,
                &CallContext::default(),
                Request::new(Command::Batch {
                    expected: None,
                    mutations: vec![set("", 1i64.into())],
                }),
            )
            .unwrap();
        let inner = state.inner.lock().unwrap();
        assert_eq!(inner.root, Some(Value::Null));
        assert_eq!(inner.revision, u64::MAX);
        // The rejected command left a readable fault but no handle at all.
        assert!(inner.handles.is_empty());
        assert!(matches!(
            State::fault(&inner, &id, &me()),
            Some(Fault::ResourceLimit { .. })
        ));
        // Faults are readable only by their principal, and repeatably.
        assert!(State::fault(&inner, &id, &other()).is_none());
        assert!(State::fault(&inner, &id, &me()).is_some());
    }

    fn empty_batch() -> Request {
        // Rejected by the pre-flight check.
        Request::new(Command::Batch {
            expected: None,
            mutations: vec![],
        })
    }

    #[tokio::test]
    async fn fault_budgets_are_per_principal_and_faults_survive_until_released() {
        let (_owner, state) = state(StateLimits::default().with_handles(2)).await;
        let base = Path::parse("").unwrap();
        let cx = CallContext::default();
        let mine = state
            .command(&base, &me(), true, &cx, empty_batch())
            .unwrap();
        // A noisy neighbour floods its own fault budget many times over.
        for _ in 0..64 {
            state
                .command(&base, &other(), true, &cx, empty_batch())
                .unwrap();
        }
        let mut inner = state.inner.lock().unwrap();
        // The neighbour holds at most its own budget; my fault is untouched.
        assert_eq!(inner.faults.count_for(&other()), 2);
        assert!(State::fault(&inner, &mine, &me()).is_some());
        // The neighbour cannot erase it either: its release is a silent no-op,
        // indistinguishable from releasing an unknown id.
        State::release(&mut inner, &mine, &other(), true).unwrap();
        assert!(State::fault(&inner, &mine, &me()).is_some());
        State::release(&mut inner, &mine, &me(), false).unwrap();
        assert!(State::fault(&inner, &mine, &me()).is_none());
    }

    #[tokio::test]
    async fn many_principals_cannot_grow_faults_past_the_global_cap() {
        let (_owner, state) =
            state(StateLimits::default().with_handles(4).with_max_faults(8)).await;
        let base = Path::parse("").unwrap();
        let cx = CallContext::default();
        let first = state
            .command(&base, &me(), true, &cx, empty_batch())
            .unwrap();
        // One fault each from many distinct principals: every one is within
        // its own budget, so only the global cap can hold the line.
        for view in 1..=100 {
            let principal = Principal { view, owner: None };
            state
                .command(&base, &principal, true, &cx, empty_batch())
                .unwrap();
            assert!(state.inner.lock().unwrap().faults.len() <= 8);
        }
        let inner = state.inner.lock().unwrap();
        assert_eq!(inner.faults.len(), 8);
        // The globally oldest went first, and reads as Closed like any
        // released handle.
        assert!(State::fault(&inner, &first, &me()).is_none());
    }

    fn snapshot() -> Request {
        Request::new(Command::Snapshot {
            prefix: String::new(),
            limits: ReadLimits::default(),
        })
    }

    #[tokio::test]
    async fn one_principal_cannot_starve_another_of_handles() {
        let (_owner, state) =
            state(StateLimits::default().with_handles(2).with_max_handles(3)).await;
        let base = Path::parse("").unwrap();
        let cx = CallContext::default();
        // The greedy principal fills its own budget and is then refused...
        for _ in 0..2 {
            state
                .command(&base, &other(), true, &cx, snapshot())
                .unwrap();
        }
        assert!(matches!(
            state.command(&base, &other(), true, &cx, snapshot()),
            Err(Error::Overloaded { .. })
        ));
        // ...while another principal still gets a handle.
        state.command(&base, &me(), true, &cx, snapshot()).unwrap();
        // The global ceiling still bounds the sum across principals.
        let third = Principal {
            view: 2,
            owner: None,
        };
        assert!(matches!(
            state.command(&base, &third, true, &cx, snapshot()),
            Err(Error::Overloaded { .. })
        ));
        assert_eq!(state.inner.lock().unwrap().handles.len(), 3);
    }

    #[tokio::test]
    async fn handles_answer_only_to_their_principal() {
        let (_owner, state) = state(StateLimits::default()).await;
        let base = Path::parse("").unwrap();
        let cx = CallContext::default();
        let receipt = state
            .command(
                &base,
                &me(),
                true,
                &cx,
                Request::new(Command::Batch {
                    expected: None,
                    mutations: vec![set("x", 1i64.into())],
                }),
            )
            .unwrap();
        // Ids are random, not a counter.
        let second = state
            .command(
                &base,
                &me(),
                true,
                &cx,
                Request::new(Command::Snapshot {
                    prefix: String::new(),
                    limits: ReadLimits::default(),
                }),
            )
            .unwrap();
        assert_ne!(
            receipt.parse::<u64>().unwrap().wrapping_add(1),
            second.parse::<u64>().unwrap()
        );
        let mut inner = state.inner.lock().unwrap();
        assert!(State::handle(&inner, &receipt, &other()).is_none());
        // A foreign release is a no-op, exactly like an unknown id.
        State::release(&mut inner, &receipt, &other(), true).unwrap();
        assert!(State::handle(&inner, &receipt, &me()).is_some());
        // Releasing a write receipt needs a writable view, even for its owner.
        assert!(State::release(&mut inner, &receipt, &me(), false).is_err());
        assert!(State::handle(&inner, &receipt, &me()).is_some());
        State::release(&mut inner, &receipt, &me(), true).unwrap();
        assert!(State::handle(&inner, &receipt, &me()).is_none());
        // Unknown ids are a no-op.
        State::release(&mut inner, "12345", &other(), false).unwrap();
    }

    #[tokio::test]
    async fn rejected_commands_do_not_consume_handle_slots() {
        // The handle budget is one. Sixty-four malformed batches must not
        // make the sixty-fifth legitimate reader wait out the handle age.
        let (_owner, state) = state(StateLimits::default().with_handles(1)).await;
        let base = Path::parse("").unwrap();
        for _ in 0..64 {
            state
                .command(
                    &base,
                    &me(),
                    true,
                    &CallContext::default(),
                    // Empty batches are rejected by the pre-flight check.
                    Request::new(Command::Batch {
                        expected: None,
                        mutations: vec![],
                    }),
                )
                .unwrap();
        }
        assert!(state.inner.lock().unwrap().handles.is_empty());
        // A legitimate reader still gets the single slot.
        state
            .command(
                &base,
                &me(),
                true,
                &CallContext::default(),
                Request::new(Command::Snapshot {
                    prefix: String::new(),
                    limits: ReadLimits::default(),
                }),
            )
            .unwrap();
        assert_eq!(state.inner.lock().unwrap().handles.len(), 1);
    }

    #[tokio::test]
    async fn batch_size_does_not_multiply_whole_tree_encoding() {
        let (_owner, state) = state(StateLimits::default()).await;
        let base = Path::parse("").unwrap();
        // A tree big enough that re-encoding it per mutation would dominate.
        let wide = Value::Map(
            (0..200)
                .map(|i| (format!("k{i}"), Value::String("x".repeat(64))))
                .collect(),
        );
        state
            .command(
                &base,
                &me(),
                true,
                &CallContext::default(),
                Request::new(Command::Batch {
                    expected: None,
                    mutations: vec![set("tree", wide)],
                }),
            )
            .unwrap();
        let tree_bytes = {
            let inner = state.inner.lock().unwrap();
            size(inner.root.as_ref().unwrap()).unwrap()
        };
        assert!(tree_bytes > 10_000, "tree should be large: {tree_bytes}");

        let mutations: Vec<_> = (0..32)
            .map(|i| set(&format!("n{i}"), 1i64.into()))
            .collect();
        counter::reset();
        state
            .command(
                &base,
                &me(),
                true,
                &CallContext::default(),
                Request::new(Command::Batch {
                    expected: None,
                    mutations,
                }),
            )
            .unwrap();
        let encoded = counter::encoded_bytes();
        // One whole-tree encoding, plus small per-mutation payloads and the
        // change record. Quadratic behaviour would be ~32 tree encodings.
        assert!(
            encoded < tree_bytes * 3,
            "32 mutations encoded {encoded} bytes against a {tree_bytes}-byte tree"
        );
    }
}
