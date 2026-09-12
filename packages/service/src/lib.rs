//! Native async services and scoped routing, independent of a Wasm engine.
//! Provider futures must be detached and dispatch must return promptly. Use
//! `BlockingStore` for blocking I/O and `ImmediateStore` only for short local work.
mod adapters;
mod admission;
mod owner;
mod retained;
pub use adapters::{BlockingStore, DetachedProvider, ImmediateStore};
pub use admission::{CallBudget, CallBudgetSnapshot, CallLimits, CallMetrics, CallUsage};
pub use owner::{
    CleanupSupervisor, CloseReport, OwnedResource, Owner, OwnerHandle, OwnerLimits, Registration,
    RemainingResource, Reservation, ResourceId, ResourceKind,
};
pub use retained::{OwnedTail, RetainedBytes, TailRead};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use structfs_core_store::{DetachedFuture, Error, Path, Record};
pub use structfs_handles::CancelToken;
use tokio::time::Instant;
use tracing::Instrument;

/// A shared reservation. Work that outlives the caller must retain a clone.
#[derive(Clone)]
pub struct Lease(Arc<dyn Send + Sync>);
impl Lease {
    pub fn new<T: Send + Sync + 'static>(reservation: T) -> Self {
        Self(Arc::new(reservation))
    }
    /// Keep this reservation alive while another owner retains the lease.
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
}
impl Default for Lease {
    fn default() -> Self {
        Self::new(())
    }
}
static NEXT: AtomicU64 = AtomicU64::new(1);
fn next_id() -> u64 {
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .expect("service identity space exhausted")
}

/// Metadata and cancellation for one request. Identity is diagnostic, not authority.
#[derive(Clone)]
pub struct CallContext {
    pub request_id: u64,
    pub cancellation: CancelToken,
    pub deadline: Option<Instant>,
    lease: Lease,
    owner: Option<OwnerHandle>,
}
impl Default for CallContext {
    fn default() -> Self {
        Self {
            request_id: next_id(),
            cancellation: CancelToken::new(),
            deadline: None,
            lease: Lease::default(),
            owner: None,
        }
    }
}
impl CallContext {
    /// The explicit owner selected by an owned provider boundary.
    pub fn owner(&self) -> Option<&OwnerHandle> {
        self.owner.as_ref()
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, Error> {
        self.deadline = Some(
            Instant::now()
                .checked_add(timeout)
                .ok_or_else(|| Error::deadline_exceeded("deadline cannot be represented"))?,
        );
        Ok(self)
    }
    /// Retain the call's accounting across a noninterruptible operation.
    pub fn lease(&self) -> Lease {
        self.lease.clone()
    }
    pub fn ensure_active(&self) -> Result<(), Error> {
        if self.cancellation.is_cancelled() {
            return Err(Error::cancelled("service call cancelled"));
        }
        if self.deadline.is_some_and(|d| d <= Instant::now()) {
            return Err(Error::deadline_exceeded("service call deadline"));
        }
        Ok(())
    }
}
/// Providers receive paths in their own namespace.
pub enum Operation {
    Read(Path),
    Write(Path, Record),
}
impl Operation {
    pub fn path(&self) -> &Path {
        match self {
            Self::Read(p) | Self::Write(p, _) => p,
        }
    }
    pub fn data(&self) -> Option<&Record> {
        match self {
            Self::Read(_) => None,
            Self::Write(_, r) => Some(r),
        }
    }
}
pub enum Response {
    Read(Option<Record>),
    Written(Path),
}
/// A native provider; dispatch constructs its future promptly without waiting for I/O.
pub trait Service: Send + Sync + 'static {
    fn call(&self, context: CallContext, operation: Operation) -> DetachedFuture<Response>;
}
/// Admission happens once before provider dispatch. Implementations may layer budgets.
pub trait Admission: Send + Sync + 'static {
    fn acquire(&self, path: &Path, data: Option<&Record>) -> Result<Lease, Error>;
}
pub struct BudgetAdmission<I: Clone + Eq + std::hash::Hash + Send + Sync + 'static = String> {
    pub budget: Arc<CallBudget<I>>,
    pub key: I,
}
impl<I: Clone + Eq + std::hash::Hash + Send + Sync + 'static> Admission for BudgetAdmission<I> {
    fn acquire(&self, p: &Path, d: Option<&Record>) -> Result<Lease, Error> {
        self.budget.acquire_record(&self.key, p, d)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
}
impl Permissions {
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
    };
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
    };
    fn intersect(self, other: Self) -> Self {
        Self {
            read: self.read && other.read,
            write: self.write && other.write,
        }
    }
}
/// Immutable, longest-component-prefix routing, also used by Featherweight wiring.
pub struct RouteTable<T> {
    entries: Vec<(Path, T)>,
}
impl<T> RouteTable<T> {
    /// Equal prefixes retain input order; the first wins. Router rejects duplicates.
    pub fn new(mut entries: Vec<(Path, T)>) -> Self {
        entries.sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        Self { entries }
    }
    pub fn resolve(&self, path: &Path) -> Option<(&T, Path, &Path)> {
        self.entries
            .iter()
            .find_map(|(p, t)| path.strip_prefix(p).map(|r| (t, r, p)))
    }
    pub fn entries(&self) -> impl Iterator<Item = (&Path, &T)> {
        self.entries.iter().map(|(p, t)| (p, t))
    }
    pub fn prefixes(&self) -> impl Iterator<Item = &Path> {
        self.entries.iter().map(|(p, _)| p)
    }
}
/// One grant from a caller-visible prefix into a provider subtree.
pub struct Mount {
    pub prefix: Path,
    pub base: Path,
    pub permissions: Permissions,
    pub service: Arc<dyn Service>,
    pub admission: Arc<dyn Admission>,
}
impl Mount {
    pub fn new(
        prefix: Path,
        base: Path,
        service: Arc<dyn Service>,
        admission: Arc<dyn Admission>,
    ) -> Self {
        Self {
            prefix,
            base,
            permissions: Permissions::READ_WRITE,
            service,
            admission,
        }
    }
}
pub struct Router {
    routes: std::sync::RwLock<RouteTable<Arc<Mount>>>,
}
impl Router {
    pub fn new(mounts: Vec<Mount>) -> Result<Arc<Self>, Error> {
        let mut seen = std::collections::BTreeSet::new();
        for m in &mounts {
            if !seen.insert(m.prefix.clone()) {
                return Err(Error::conflict("duplicate service mount"));
            }
        }
        Ok(Arc::new(Self {
            routes: std::sync::RwLock::new(RouteTable::new(
                mounts
                    .into_iter()
                    .map(|m| (m.prefix.clone(), Arc::new(m)))
                    .collect(),
            )),
        }))
    }
    pub fn client(self: &Arc<Self>) -> Client {
        Client {
            router: self.clone(),
            base: Path::parse("").unwrap(),
            permissions: Permissions::READ_WRITE,
            context: None,
            owners: Vec::new(),
        }
    }
    /// Install an owned mount. Revocation is immediate; removal and cleanup are
    /// supervised. Equal prefixes must be removed before registering a replacement.
    pub fn register(
        self: &Arc<Self>,
        owner: &OwnerHandle,
        mut mount: Mount,
    ) -> Result<Registration, Error> {
        let mut routes = self.routes.write().unwrap_or_else(|e| e.into_inner());
        if routes.prefixes().any(|p| p == &mount.prefix) {
            return Err(Error::conflict("duplicate service mount"));
        }
        let router = Arc::downgrade(self);
        let prefix = mount.prefix.clone();
        let registration = owner.register(ResourceKind::Registration, 0, move || async move {
            if let Some(router) = router.upgrade() {
                router
                    .routes
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .entries
                    .retain(|(p, _)| p != &prefix);
            }
            Ok(())
        })?;
        mount.service = Arc::new(OwnedService {
            owner: owner.clone(),
            service: mount.service,
            revoked: registration.cancellation(),
        });
        routes.entries.push((mount.prefix.clone(), Arc::new(mount)));
        routes
            .entries
            .sort_by_key(|(p, _)| std::cmp::Reverse(p.len()));
        Ok(registration)
    }
}
/// Cloneable capability handle. Scoping and permissions can only be attenuated.
#[derive(Clone)]
pub struct Client {
    router: Arc<Router>,
    base: Path,
    permissions: Permissions,
    context: Option<CallContext>,
    owners: Vec<OwnerHandle>,
}
impl Client {
    /// Bind calls to an additional lifetime. Cloning, scoping, or replacing
    /// metadata cannot remove an existing owner constraint.
    pub fn owned_by(&self, owner: &OwnerHandle) -> Self {
        let mut client = self.clone();
        if !client.owners.iter().any(|o| o.id() == owner.id()) {
            client.owners.push(owner.clone());
        }
        client
    }
    pub fn scoped(&self, base: &Path, permissions: Permissions) -> Self {
        Self {
            base: self.base.join(base),
            permissions: self.permissions.intersect(permissions),
            ..self.clone()
        }
    }
    pub fn with_context(&self, context: CallContext) -> Self {
        Self {
            context: Some(context),
            ..self.clone()
        }
    }
    pub async fn read(&self, path: &Path) -> Result<Option<Record>, Error> {
        match self.call(Operation::Read(path.clone())).await? {
            Response::Read(r) => Ok(r),
            _ => Err(Error::store(
                "service",
                "read",
                "provider returned a write response",
            )),
        }
    }
    pub async fn write(&self, path: &Path, data: Record) -> Result<Path, Error> {
        match self.call(Operation::Write(path.clone(), data)).await? {
            Response::Written(p) => Ok(p),
            _ => Err(Error::store(
                "service",
                "write",
                "provider returned a read response",
            )),
        }
    }
    async fn call(&self, operation: Operation) -> Result<Response, Error> {
        let parent = self.context.clone().unwrap_or_default();
        parent.ensure_active()?;
        for owner in &self.owners {
            owner.ensure_open()?;
        }
        let is_read = matches!(operation, Operation::Read(_));
        let absolute = self.base.join(operation.path());
        let resolved = self
            .router
            .routes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .resolve(&absolute)
            .map(|(m, r, p)| (m.clone(), r, p.clone()));
        let (mount, relative, prefix) = resolved.ok_or_else(|| {
            Error::permission_denied(format!(
                "path is not wired into this namespace: {}",
                operation.path()
            ))
        })?;
        let permissions = self.permissions.intersect(mount.permissions);
        if !(if is_read {
            permissions.read
        } else {
            permissions.write
        }) {
            return Err(Error::permission_denied("operation is not granted"));
        }
        let mapped = mount.base.join(&relative);
        let lease = mount.admission.acquire(&mapped, operation.data())?;
        parent.ensure_active()?;
        let cancellation = CancelToken::new();
        struct CancelOnDrop(CancelToken);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let _guard = CancelOnDrop(cancellation.clone());
        let _reservation = lease.clone();
        let context = CallContext {
            cancellation,
            lease,
            ..parent.clone()
        };
        let op = match operation {
            Operation::Read(_) => Operation::Read(mapped),
            Operation::Write(_, data) => Operation::Write(mapped, data),
        };
        let mut service = mount.service.clone();
        for owner in self.owners.iter().rev() {
            service = owner.service(service);
        }
        let future = service.call(context, op);
        let deadline = async {
            match parent.deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        let span = tracing::info_span!("structfs.call",request_id=parent.request_id,mount=%prefix,operation=if is_read{"read"}else{"write"});
        let result = async {
            tokio::select! {biased;
                _=parent.cancellation.cancelled()=>Err(Error::cancelled("service call cancelled")),
                _=deadline=>Err(Error::deadline_exceeded("service call deadline")),
                result=future=>result,
            }
        }
        .instrument(span)
        .await?;
        match result {
            Response::Read(_) if !is_read => Err(Error::store(
                "service",
                "write",
                "provider response kind mismatch",
            )),
            Response::Written(_) if is_read => Err(Error::store(
                "service",
                "read",
                "provider response kind mismatch",
            )),
            Response::Written(path) => {
                let relative = path
                    .strip_prefix(&mount.base)
                    .ok_or_else(|| Error::permission_denied("provider result escaped grant"))?;
                let path = prefix
                    .join(&relative)
                    .strip_prefix(&self.base)
                    .ok_or_else(|| {
                        Error::permission_denied("provider result escaped client scope")
                    })?;
                Ok(Response::Written(path))
            }
            response => Ok(response),
        }
    }
}
#[async_trait::async_trait]
impl structfs_core_store::AsyncReader for Client {
    async fn read_async(&mut self, p: &Path) -> Result<Option<Record>, Error> {
        self.read(p).await
    }
}
#[async_trait::async_trait]
impl structfs_core_store::AsyncWriter for Client {
    async fn write_async(&mut self, p: &Path, d: Record) -> Result<Path, Error> {
        self.write(p, d).await
    }
}
impl structfs_core_store::DetachedReader for Client {
    fn read_detached(&mut self, p: &Path) -> DetachedFuture<Option<Record>> {
        let this = self.clone();
        let p = p.clone();
        Box::pin(async move { this.read(&p).await })
    }
}
impl structfs_core_store::DetachedWriter for Client {
    fn write_detached(&mut self, p: &Path, d: Record) -> DetachedFuture<Path> {
        let this = self.clone();
        let p = p.clone();
        Box::pin(async move { this.write(&p, d).await })
    }
}

/// Apply an explicit lifetime to a provider, including Featherweight imports
/// passed through `service_host_store`. In-flight blocking work retains both
/// admission and ownership through the context lease.
struct OwnedService {
    owner: OwnerHandle,
    service: Arc<dyn Service>,
    revoked: CancelToken,
}
impl OwnerHandle {
    pub fn service(&self, service: Arc<dyn Service>) -> Arc<dyn Service> {
        Arc::new(OwnedService {
            owner: self.clone(),
            service,
            revoked: CancelToken::new(),
        })
    }
}
impl Service for OwnedService {
    fn call(&self, mut context: CallContext, operation: Operation) -> DetachedFuture<Response> {
        let owner = self.owner.clone();
        let service = self.service.clone();
        let revoked = self.revoked.clone();
        Box::pin(async move {
            if revoked.is_cancelled() {
                return Err(Error::cancelled("registration revoked"));
            }
            context.ensure_active()?;
            let reservation = owner.track(ResourceKind::Provider, 0)?;
            context.lease = Lease::new((context.lease(), reservation));
            if context.owner.is_none() {
                context.owner = Some(owner.clone());
            }
            let _lease = context.lease();
            let cancellation = owner.cancellation();
            let parent_cancel = context.cancellation.clone();
            let call_cancel = CancelToken::new();
            context.cancellation = call_cancel.clone();
            let deadline = context.deadline;
            struct CancelOnDrop(CancelToken);
            impl Drop for CancelOnDrop {
                fn drop(&mut self) {
                    self.0.cancel();
                }
            }
            let _cancel = CancelOnDrop(call_cancel);
            owner.ensure_open()?;
            let future = service.call(context, operation);
            let expiry = async move {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! { biased;
                _ = revoked.cancelled() => Err(Error::cancelled("registration revoked")),
                _ = cancellation.cancelled() => Err(Error::cancelled("owner closed")),
                _ = parent_cancel.cancelled() => Err(Error::cancelled("service call cancelled")),
                _ = expiry => Err(Error::deadline_exceeded("service call deadline")),
                result = future => result,
            }
        })
    }
}
