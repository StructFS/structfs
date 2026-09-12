use crate::*;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use structfs_core_store::{Codec, DetachedFuture, Error, Path, Record};
use structfs_serde_store::{from_value, to_value, Profile, ValueCodec};
use structfs_service::{
    CallContext, Operation, OwnerHandle, Registration, ResourceKind, Response, Service,
};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone, Debug)]
pub struct StateLimits {
    pub state_bytes: usize,
    pub history_bytes: usize,
    pub history_records: usize,
    pub change_bytes: usize,
    pub snapshot_bytes: usize,
    pub total_snapshot_bytes: usize,
    pub handles: usize,
    pub mutations: usize,
    pub depth: usize,
    pub nodes: usize,
    pub page_bytes: usize,
    pub page_items: usize,
    pub handle_age: Duration,
}
impl Default for StateLimits {
    fn default() -> Self {
        Self {
            state_bytes: 1 << 20,
            history_bytes: 1 << 20,
            history_records: 256,
            change_bytes: 32768,
            snapshot_bytes: 2 << 20,
            total_snapshot_bytes: 8 << 20,
            handles: 64,
            mutations: 128,
            depth: 32,
            nodes: 32768,
            page_bytes: 65536,
            page_items: 256,
            handle_age: Duration::from_secs(60),
        }
    }
}
fn invalid(s: impl Into<String>) -> Fault {
    Fault::Invalid { message: s.into() }
}
fn limit(s: impl Into<String>) -> Fault {
    Fault::ResourceLimit { message: s.into() }
}
fn path(s: &str) -> Result<Path, Fault> {
    Path::parse(s).map_err(|_| invalid("invalid relative path"))
}
fn size<T: Serialize>(v: &T) -> Result<usize, Fault> {
    let value = to_value(v).map_err(|_| limit("value conversion bounds"))?;
    let codec = ValueCodec::new(Profile::ValueJson).canonical();
    codec
        .encode(&value, &codec.profile.format())
        .map(|b| b.len())
        .map_err(|_| limit("encoded value bounds"))
}
fn reply<T: Serialize>(result: Result<T, Fault>) -> Result<Record, Error> {
    to_value(&match result {
        Ok(v) => Reply::Ok(v),
        Err(e) => Reply::Error(e),
    })
    .map(Record::parsed)
}
struct History {
    change: Change,
    bytes: usize,
}
struct Handle {
    base: Path,
    prefix: Path,
    descriptor: Descriptor,
    nodes: Vec<Node>,
    snapshot_bytes: usize,
    limits: ReadLimits,
    expires: Instant,
    registration: Registration,
    failure: Option<Fault>,
}
struct Inner {
    root: Option<Value>,
    revision: u64,
    history: VecDeque<History>,
    history_bytes: usize,
    handles: BTreeMap<String, Handle>,
    snapshot_bytes: usize,
    closed: bool,
}
/// In-memory durability only. Create a restricted `view` for each granted subtree.
/// Command payload paths are relative to that view, never global router paths.
pub struct State {
    inner: Mutex<Inner>,
    epoch: String,
    limits: StateLimits,
    owner: OwnerHandle,
    notify: Notify,
    lifetime: Mutex<Option<Registration>>,
}
struct View {
    state: Arc<State>,
    base: Path,
    writable: bool,
}
impl State {
    pub fn new(
        owner: &OwnerHandle,
        initial: Option<Value>,
        limits: StateLimits,
    ) -> Result<Arc<Self>, Error> {
        if limits.history_records == 0
            || limits.change_bytes == 0
            || limits.change_bytes > limits.history_bytes
            || limits.page_bytes < limits.change_bytes.saturating_add(512)
            || limits.page_items == 0
            || limits.depth > 48
            || limits.handle_age.is_zero()
            || Instant::now().checked_add(limits.handle_age).is_none()
        {
            return Err(Error::resource_limit("incompatible state limits"));
        }
        if let Some(v) = &initial {
            validate(v, &limits).map_err(|e| Error::resource_limit(e.to_string()))?;
        }
        let state = Arc::new(Self {
            inner: Mutex::new(Inner {
                root: initial,
                revision: 0,
                history: VecDeque::new(),
                history_bytes: 0,
                handles: BTreeMap::new(),
                snapshot_bytes: 0,
                closed: false,
            }),
            epoch: uuid::Uuid::new_v4().to_string(),
            limits,
            owner: owner.clone(),
            notify: Notify::new(),
            lifetime: Mutex::new(None),
        });
        let weak = Arc::downgrade(&state);
        let reserved = state
            .limits
            .state_bytes
            .checked_mul(2)
            .and_then(|n| n.checked_add(state.limits.history_bytes))
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
                    inner.snapshot_bytes = 0;
                    drop(inner);
                    s.notify.notify_waiters();
                }
                Ok(())
            })?;
        *state.lifetime.lock().unwrap_or_else(|e| e.into_inner()) = Some(registration);
        Ok(state)
    }
    pub fn view(self: &Arc<Self>, base: Path, writable: bool) -> Arc<dyn Service> {
        Arc::new(View {
            state: self.clone(),
            base,
            writable,
        })
    }
    pub fn token(&self) -> Token {
        self.token_at(
            self.inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .revision,
        )
    }
    fn token_at(&self, revision: u64) -> Token {
        Token {
            epoch: self.epoch.clone(),
            revision,
        }
    }
    fn remove(inner: &mut Inner, id: &str) {
        if let Some(h) = inner.handles.remove(id) {
            inner.snapshot_bytes -= h.snapshot_bytes;
        }
    }
    fn reap(inner: &mut Inner) {
        let now = Instant::now();
        let ids: Vec<_> = inner
            .handles
            .iter()
            .filter(|(_, h)| h.expires <= now || h.registration.cancellation().is_cancelled())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            Self::remove(inner, &id);
        }
    }
    fn check_token(&self, inner: &Inner, after: &Token) -> Result<(), Fault> {
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
    fn read_limits(&self, l: &ReadLimits) -> Result<(), Fault> {
        if l.page_items == 0
            || l.page_items > self.limits.page_items
            || l.page_bytes > self.limits.page_bytes
            || l.page_bytes < self.limits.change_bytes.saturating_add(512)
        {
            return Err(limit("incompatible page limits"));
        }
        Ok(())
    }
    fn command(
        self: &Arc<Self>,
        base: &Path,
        writable: bool,
        context: &CallContext,
        request: Request,
    ) -> Result<String, Error> {
        context.ensure_active()?;
        self.owner.ensure_open()?;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::reap(&mut inner);
        if inner.closed {
            return Err(Error::cancelled("state closed"));
        }
        if inner.handles.len() >= self.limits.handles {
            return Err(Error::overloaded("state handle limit"));
        }
        let id = format!("h{}", uuid::Uuid::new_v4().simple());
        let expires = Instant::now()
            .checked_add(self.limits.handle_age)
            .ok_or_else(|| Error::resource_limit("handle age overflow"))?;
        let current = self.token_at(inner.revision);
        // Build and validate the proposed publication without changing live state.
        let prepared = (|| -> Result<Prepared, Fault> {
            if request.version != 1 {
                return Err(invalid("unsupported state protocol version"));
            }
            match request.command {
                Command::Batch {
                    expected,
                    mutations,
                } => {
                    if !writable {
                        return Err(invalid("view is read only"));
                    }
                    if let Some(expected) = expected {
                        if expected.epoch != self.epoch {
                            return Err(Fault::EpochMismatch {
                                current: current.clone(),
                            });
                        }
                        if expected != current {
                            return Err(Fault::Conflict {
                                current: current.clone(),
                            });
                        }
                    }
                    if mutations.is_empty() || mutations.len() > self.limits.mutations {
                        return Err(limit("batch count"));
                    }
                    let revision = inner
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| limit("revision exhausted"))?;
                    let mut root = inner.root.clone();
                    let mut touched = BTreeSet::new();
                    for mutation in mutations {
                        let relative = path(mutation.path())?;
                        let target = base.join(&relative);
                        if target.len() > self.limits.depth {
                            return Err(limit("mutation path depth"));
                        }
                        // Conservative parent invalidation also covers array index shifts.
                        touched.insert(target.slice(0, target.len().saturating_sub(1)).to_string());
                        match mutation {
                            Mutation::Set { value, .. } => {
                                validate(&value, &self.limits)?;
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
                        if let Some(v) = &root {
                            validate(v, &self.limits)?;
                        }
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
                        bytes: size(&base.to_string())?.saturating_add(512),
                        limits: ReadLimits::default(),
                        commit: Some((root, History { change, bytes })),
                    })
                }
                command => {
                    let (relative, limits, snapshot, after) = match command {
                        Command::Snapshot { prefix, limits } => (prefix, limits, true, None),
                        Command::Observe { prefix, limits } => {
                            (prefix, limits, true, Some(current.clone()))
                        }
                        Command::Watch {
                            prefix,
                            after,
                            limits,
                        } => (prefix, limits, false, Some(after)),
                        _ => unreachable!(),
                    };
                    self.read_limits(&limits)?;
                    if let Some(t) = &after {
                        self.check_token(&inner, t)?;
                    }
                    let prefix = base.join(&path(&relative)?);
                    let nodes = if snapshot {
                        flatten(
                            inner.root.as_ref().and_then(|v| v.get(&prefix)),
                            &self.limits,
                        )?
                    } else {
                        vec![]
                    };
                    let bytes = size(&nodes)?
                        .saturating_add(size(&prefix.to_string())?)
                        .saturating_add(512);
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
                        if size(node)?.saturating_add(512) > limits.page_bytes {
                            return Err(limit("snapshot node cannot fit page"));
                        }
                    }
                    Ok(Prepared {
                        descriptor: Descriptor {
                            token: after.clone().unwrap_or(current),
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
        })();
        let (prepared, failure) = match prepared {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        let bytes = prepared
            .as_ref()
            .map_or_else(|| base.to_string().len().saturating_add(512), |p| p.bytes);
        if bytes
            > self
                .limits
                .total_snapshot_bytes
                .saturating_sub(inner.snapshot_bytes)
        {
            return Err(Error::overloaded("retained handle bytes"));
        }
        let weak: Weak<Self> = Arc::downgrade(self);
        let cleanup_id = id.clone();
        let owner = context.owner().unwrap_or(&self.owner);
        let registration = owner.register(ResourceKind::Retained, bytes, move || async move {
            if let Some(state) = weak.upgrade() {
                Self::remove(
                    &mut state.inner.lock().unwrap_or_else(|e| e.into_inner()),
                    &cleanup_id,
                );
                state.notify.notify_waiters();
            }
            Ok(())
        })?;
        context.ensure_active()?;
        owner.ensure_open()?;
        self.owner.ensure_open()?;
        let (descriptor, prefix, nodes, limits, commit) = match prepared {
            Some(p) => (p.descriptor, p.prefix, p.nodes, p.limits, p.commit),
            None => (
                Descriptor {
                    token: self.token_at(inner.revision),
                    snapshot: false,
                    watch: false,
                    committed: false,
                },
                base.clone(),
                vec![],
                ReadLimits::default(),
                None,
            ),
        };
        if let Some((root, history)) = commit {
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
        inner.snapshot_bytes += bytes;
        inner.handles.insert(
            id.clone(),
            Handle {
                base: base.clone(),
                prefix,
                descriptor,
                nodes,
                snapshot_bytes: bytes,
                limits,
                expires,
                registration,
                failure,
            },
        );
        drop(inner);
        self.notify.notify_waiters();
        Ok(id)
    }
}
struct Prepared {
    descriptor: Descriptor,
    prefix: Path,
    nodes: Vec<Node>,
    bytes: usize,
    limits: ReadLimits,
    commit: Option<(Option<Value>, History)>,
}
fn validate(value: &Value, limits: &StateLimits) -> Result<(), Fault> {
    fn visit(value: &Value, depth: usize, count: &mut usize, l: &StateLimits) -> Result<(), Fault> {
        *count += 1;
        if depth > l.depth || *count > l.nodes {
            return Err(limit("state depth or node count"));
        }
        match value {
            Value::Map(m) => {
                for v in m.values() {
                    visit(v, depth + 1, count, l)?;
                }
            }
            Value::Array(a) => {
                for v in a {
                    visit(v, depth + 1, count, l)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    visit(value, 0, &mut 0, limits)?;
    if size(value)? > limits.state_bytes {
        return Err(limit("state bytes"));
    }
    Ok(())
}
fn flatten(value: Option<&Value>, limits: &StateLimits) -> Result<Vec<Node>, Fault> {
    fn visit(
        v: &Value,
        path: &mut Vec<String>,
        out: &mut Vec<Node>,
        used: &mut usize,
        l: &StateLimits,
    ) -> Result<(), Fault> {
        if out.len() >= l.nodes {
            return Err(limit("snapshot nodes"));
        }
        if path.iter().map(String::len).sum::<usize>() > l.snapshot_bytes.saturating_sub(*used) {
            return Err(limit("snapshot path bytes"));
        }
        let skeleton = match v {
            Value::Map(_) => Value::Map(BTreeMap::new()),
            Value::Array(_) => Value::Array(vec![]),
            other => other.clone(),
        };
        let node = Node {
            path: path.clone(),
            value: skeleton,
        };
        let bytes = size(&node)?;
        if bytes > l.snapshot_bytes.saturating_sub(*used) {
            return Err(limit("snapshot bytes"));
        }
        *used += bytes;
        out.push(node);
        match v {
            Value::Map(m) => {
                for (k, v) in m {
                    path.push(k.clone());
                    visit(v, path, out, used, l)?;
                    path.pop();
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    path.push(i.to_string());
                    visit(v, path, out, used, l)?;
                    path.pop();
                }
            }
            _ => (),
        }
        Ok(())
    }
    let mut nodes = vec![];
    if let Some(v) = value {
        visit(v, &mut vec![], &mut nodes, &mut 0, limits)?;
    }
    Ok(nodes)
}
impl View {
    async fn read(&self, context: &CallContext, p: &Path) -> Result<Option<Record>, Error> {
        if p.is_empty() {
            return to_value(&Capabilities {
                version: 1,
                durability: "memory".into(),
                max_change_bytes: self.state.limits.change_bytes,
                min_page_bytes: self.state.limits.change_bytes + 512,
                max_page_bytes: self.state.limits.page_bytes,
                max_page_items: self.state.limits.page_items,
            })
            .map(Record::parsed)
            .map(Some);
        }
        if &p[0] == "data" {
            context.ensure_active()?;
            self.state.owner.ensure_open()?;
            let inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            return Ok(inner
                .root
                .as_ref()
                .and_then(|v| v.get(&self.base.join(&p.slice(1, p.len()))))
                .cloned()
                .map(Record::parsed));
        }
        if p.len() < 2 || &p[0] != "outstanding" {
            return Err(Error::permission_denied("state read surface"));
        }
        loop {
            let notified = self.state.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let expiry = {
                context.ensure_active()?;
                self.state.owner.ensure_open()?;
                let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                State::reap(&mut inner);
                let Some(h) = inner.handles.get(&p[1]).filter(|h| h.base == self.base) else {
                    return reply::<Descriptor>(Err(Fault::Closed)).map(Some);
                };
                if let Some(fault) = &h.failure {
                    return reply::<Descriptor>(Err(fault.clone())).map(Some);
                }
                if p.len() == 2 {
                    return reply(Ok(h.descriptor.clone())).map(Some);
                }
                if p.len() != 4 {
                    return Err(Error::permission_denied("state handle path"));
                }
                let cursor: u64 = p[3]
                    .parse()
                    .map_err(|_| Error::conflict("invalid cursor"))?;
                if &p[2] == "snapshot" && h.descriptor.snapshot {
                    if cursor > h.nodes.len() as u64 {
                        return reply::<SnapshotPage>(Err(invalid("snapshot cursor"))).map(Some);
                    }
                    let mut page = SnapshotPage {
                        token: h.descriptor.token.clone(),
                        items: vec![],
                        next: cursor,
                        done: cursor == h.nodes.len() as u64,
                    };
                    for node in h
                        .nodes
                        .iter()
                        .skip(cursor as usize)
                        .take(h.limits.page_items)
                    {
                        page.items.push(node.clone());
                        page.next += 1;
                        page.done = page.next == h.nodes.len() as u64;
                        if size(&Reply::Ok(&page))
                            .map_err(|e| Error::resource_limit(e.to_string()))?
                            > h.limits.page_bytes
                        {
                            page.items.pop();
                            page.next -= 1;
                            page.done = false;
                            break;
                        }
                    }
                    if page.items.is_empty() && !page.done {
                        return reply::<SnapshotPage>(Err(limit("snapshot page"))).map(Some);
                    }
                    return reply(Ok(page)).map(Some);
                }
                if &p[2] != "changes" || !h.descriptor.watch {
                    return Err(Error::permission_denied("state handle operation"));
                }
                if cursor < h.descriptor.token.revision {
                    return reply::<ChangePage>(Err(invalid("cursor precedes watch grant")))
                        .map(Some);
                }
                if let Err(e) = self.state.check_token(&inner, &self.state.token_at(cursor)) {
                    return reply::<ChangePage>(Err(e)).map(Some);
                }
                let mut page = ChangePage {
                    items: vec![],
                    next: self.state.token_at(cursor),
                    done: false,
                };
                for history in inner
                    .history
                    .iter()
                    .filter(|c| c.change.token.revision > cursor)
                {
                    let paths: Vec<String> = history
                        .change
                        .paths
                        .iter()
                        .filter_map(|p| {
                            let p = Path::parse(p).expect("validated commit path");
                            if let Some(relative) = p.strip_prefix(&h.prefix) {
                                Some(relative.to_string())
                            } else if h.prefix.strip_prefix(&p).is_some() {
                                Some(String::new())
                            } else {
                                None
                            }
                        })
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    if paths.is_empty() {
                        page.next = history.change.token.clone();
                        continue;
                    }
                    if page.items.len() == h.limits.page_items {
                        break;
                    }
                    let previous = page.next.clone();
                    page.items.push(Change {
                        token: history.change.token.clone(),
                        paths,
                    });
                    page.next = history.change.token.clone();
                    if size(&Reply::Ok(&page)).map_err(|e| Error::resource_limit(e.to_string()))?
                        > h.limits.page_bytes
                    {
                        page.items.pop();
                        page.next = previous;
                        break;
                    }
                }
                // Even a filtered empty page advances the cursor, avoiding repeated scans.
                if page.next.revision != cursor {
                    return reply(Ok(page)).map(Some);
                }
                if cursor < inner.revision {
                    return reply::<ChangePage>(Err(limit("change page"))).map(Some);
                }
                h.expires
            };
            tokio::select! { biased;
                _ = context.cancellation.cancelled() => return Err(Error::cancelled("state watch cancelled")),
                _ = tokio::time::sleep_until(context.deadline.map_or(expiry, |d| d.min(expiry))) => {
                    context.ensure_active()?;
                    // Recheck expiry and return Closed through the normal handle path.
                },
                _ = notified => {},
            }
        }
    }
}
impl Service for View {
    fn call(&self, context: CallContext, operation: Operation) -> DetachedFuture<Response> {
        let this = Self {
            state: self.state.clone(),
            base: self.base.clone(),
            writable: self.writable,
        };
        Box::pin(async move {
            match operation {
                Operation::Read(p) => this.read(&context, &p).await.map(Response::Read),
                Operation::Write(p, r) => {
                    context.ensure_active()?;
                    this.state.owner.ensure_open()?;
                    if p == structfs_core_store::path!("operations") {
                        let value = r.into_value(&structfs_core_store::NoCodec)?;
                        // Bound before recursive Serde conversion; arbitrary Raw records fail.
                        validate(&value, &this.state.limits)
                            .map_err(|e| Error::resource_limit(e.to_string()))?;
                        let request: Request = from_value(value)?;
                        let id =
                            this.state
                                .command(&this.base, this.writable, &context, request)?;
                        return Ok(Response::Written(
                            Path::parse(&format!("outstanding/{id}")).unwrap(),
                        ));
                    }
                    if p.len() == 3 && &p[0] == "outstanding" && &p[2] == "release" {
                        let mut inner = this.state.inner.lock().unwrap_or_else(|e| e.into_inner());
                        if inner
                            .handles
                            .get(&p[1])
                            .is_some_and(|h| h.base != this.base)
                        {
                            return Err(Error::permission_denied("handle grant"));
                        }
                        State::remove(&mut inner, &p[1]);
                        drop(inner);
                        this.state.notify.notify_waiters();
                        return Ok(Response::Written(p.slice(0, 2)));
                    }
                    Err(Error::permission_denied(
                        "state data is read only; use operations",
                    ))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn revision_overflow_fails_before_commit() {
        let supervisor = structfs_service::CleanupSupervisor::new(1).unwrap();
        let owner = supervisor.owner(Default::default()).unwrap();
        let state = State::new(&owner.handle(), Some(Value::Null), StateLimits::default()).unwrap();
        state.inner.lock().unwrap().revision = u64::MAX;
        let id = state
            .command(
                &Path::parse("").unwrap(),
                true,
                &CallContext::default(),
                Request::new(Command::Batch {
                    expected: None,
                    mutations: vec![Mutation::Set {
                        path: "".into(),
                        value: 1i64.into(),
                    }],
                }),
            )
            .unwrap();
        let inner = state.inner.lock().unwrap();
        assert_eq!(inner.root, Some(Value::Null));
        assert_eq!(inner.revision, u64::MAX);
        assert!(matches!(
            inner.handles[&id].failure,
            Some(Fault::ResourceLimit { .. })
        ));
    }
}
