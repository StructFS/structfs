//! The `Service` surface over a [`State`]: one restricted subtree per view.

use std::sync::Arc;

use structfs_core_store::{path, Error, NoCodec, Path, Record};
use structfs_serde_store::{from_value, to_value};
use structfs_service::{CallContext, Operation, Response, Service};
use tokio::time::Instant;

use crate::limits::validate;
use crate::paging::{project_change, snapshot_page, ChangePageBuilder};
use crate::provider::{Inner, Principal, State};
use crate::{Capabilities, ChangePage, Descriptor, Fault, Reply, Request, SnapshotPage};

pub(crate) struct View {
    state: Arc<State>,
    id: u64,
    base: Path,
    writable: bool,
}

/// What one evaluation of a handle read decided.
enum Step {
    /// A reply is ready.
    Reply(Result<Option<Record>, Error>),
    /// Nothing to report yet; park until a commit or `expires`, whichever is first.
    Park(Instant),
}

fn reply<T: serde::Serialize>(result: Result<T, Fault>) -> Result<Option<Record>, Error> {
    to_value(&match result {
        Ok(v) => Reply::Ok(v),
        Err(e) => Reply::Error(e),
    })
    .map(Record::parsed)
    .map(Some)
}

impl View {
    pub(crate) fn new(state: Arc<State>, id: u64, base: Path, writable: bool) -> Self {
        Self {
            state,
            id,
            base,
            writable,
        }
    }

    /// The caller: this view plus the owner the call is bound to, if any.
    fn principal(&self, context: &CallContext) -> Principal {
        Principal {
            view: self.id,
            owner: context.owner().map(|o| o.id()),
        }
    }

    fn capabilities(&self) -> Result<Option<Record>, Error> {
        to_value(&Capabilities {
            version: 1,
            durability: "memory".into(),
            max_change_bytes: self.state.limits.change_bytes,
            min_page_bytes: self.state.limits.change_bytes + crate::limits::PAGE_OVERHEAD,
            max_page_bytes: self.state.limits.page_bytes,
            max_page_items: self.state.limits.page_items,
        })
        .map(Record::parsed)
        .map(Some)
    }

    fn data(&self, context: &CallContext, p: &Path) -> Result<Option<Record>, Error> {
        context.ensure_active()?;
        self.state.owner.ensure_open()?;
        let inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        Ok(inner
            .root
            .as_ref()
            .and_then(|v| v.get(&self.base.join(&p.slice(1, p.len()))))
            .cloned()
            .map(Record::parsed))
    }

    /// Evaluate one handle read against the current state.
    ///
    /// Everything here is synchronous and holds the state lock only for the
    /// duration of the evaluation; parking happens outside, on the state's
    /// gate, so the enable-before-check ordering is the gate's problem and not
    /// a hand-rolled `Notify` dance.
    fn step(&self, context: &CallContext, p: &Path) -> Result<Step, Error> {
        context.ensure_active()?;
        self.state.owner.ensure_open()?;
        let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
        State::reap(&mut inner);
        let principal = self.principal(context);
        if let Some(fault) = State::fault(&inner, &p[1], &principal) {
            return Ok(Step::Reply(reply::<Descriptor>(Err(fault))));
        }
        // Another principal's handle reads exactly like an unknown one (and
        // releasing it is the same silent no-op), so probing ids reveals
        // nothing.
        let Some(handle) = State::handle(&inner, &p[1], &principal) else {
            return Ok(Step::Reply(reply::<Descriptor>(Err(Fault::Closed))));
        };
        if p.len() == 2 {
            return Ok(Step::Reply(reply(Ok(handle.descriptor.clone()))));
        }
        if p.len() != 4 {
            return Err(Error::not_found(p.clone()));
        }
        let cursor: u64 = p[3]
            .parse()
            .map_err(|_| Error::invalid_argument("state cursor must be a number"))?;

        if &p[2] == "snapshot" && handle.descriptor.snapshot {
            return Ok(Step::Reply(reply::<SnapshotPage>(snapshot_page(
                &handle.descriptor.token,
                &handle.nodes,
                cursor,
                &handle.limits,
            ))));
        }
        if &p[2] != "changes" || !handle.descriptor.watch {
            return Err(Error::permission_denied("state handle operation"));
        }
        if cursor < handle.descriptor.token.revision {
            return Ok(Step::Reply(reply::<ChangePage>(Err(
                crate::limits::invalid("cursor precedes watch grant"),
            ))));
        }
        // The path carries only a revision, so this checks retention and
        // future cursors; the epoch of the caller's token is compared by
        // `StateHandle::changes` against the epoch it was granted.
        if let Err(e) = self.state.check_token(&inner, &self.state.token_at(cursor)) {
            return Ok(Step::Reply(reply::<ChangePage>(Err(e))));
        }
        match self.change_page(&inner, &p[1], cursor)? {
            Some(page) => Ok(Step::Reply(reply(Ok(page)))),
            None => Ok(Step::Park(inner.handles[&p[1]].expires)),
        }
    }

    /// Assemble a change page, or `None` when the caller is already current
    /// and should park.
    fn change_page(
        &self,
        inner: &Inner,
        id: &str,
        cursor: u64,
    ) -> Result<Option<ChangePage>, Error> {
        let handle = &inner.handles[id];
        let mut builder =
            ChangePageBuilder::new(self.state.token_at(cursor), handle.limits.clone());
        for history in inner
            .history
            .iter()
            .filter(|c| c.change.token.revision > cursor)
        {
            match project_change(&history.change, &handle.prefix) {
                // A commit this handle cannot see still advances the cursor,
                // so a filtered watch does not rescan the same history.
                None => builder.skip_to(history.change.token.clone()),
                Some(change) => {
                    let fitted = builder
                        .push(change)
                        .map_err(|e| Error::resource_limit(e.to_string()))?;
                    if !fitted {
                        break;
                    }
                }
            }
        }
        if builder.cursor().revision != cursor {
            return Ok(Some(builder.finish()));
        }
        if cursor < inner.revision {
            // Every retained commit is invisible to this handle, yet the
            // cursor cannot advance: the page budget cannot fit even one.
            return Err(Error::resource_limit("change page"));
        }
        Ok(None)
    }

    async fn read(&self, context: &CallContext, p: &Path) -> Result<Option<Record>, Error> {
        if p.is_empty() {
            return self.capabilities();
        }
        if &p[0] == "data" {
            return self.data(context, p);
        }
        if p.len() < 2 || &p[0] != "outstanding" {
            return Err(Error::permission_denied("state read surface"));
        }
        loop {
            let expiry = match self.step(context, p)? {
                Step::Reply(reply) => return reply,
                Step::Park(expiry) => expiry,
            };
            let deadline = context.deadline.map_or(expiry, |d| d.min(expiry));
            // The gate re-runs `step` on every commit with enable-before-check
            // ordering, so a commit racing the evaluation is never lost.
            let parked =
                self.state
                    .gate
                    .wait_until_cancellable(&context.cancellation, || {
                        match self.step(context, p) {
                            Ok(Step::Park(_)) => None,
                            Ok(Step::Reply(reply)) => Some(reply),
                            Err(error) => Some(Err(error)),
                        }
                    });
            tokio::select! { biased;
                _ = tokio::time::sleep_until(deadline) => {
                    // Expiry or caller deadline: re-evaluate, which reports
                    // Closed through the ordinary handle path.
                    context.ensure_active()?;
                }
                reply = parked => {
                    return reply.map_err(|e| e.into_error("state watch cancelled"))?;
                }
            }
        }
    }

    fn write(&self, context: &CallContext, p: Path, data: Record) -> Result<Response, Error> {
        context.ensure_active()?;
        self.state.owner.ensure_open()?;
        if p == path!("operations") {
            let value = data.into_value(&NoCodec)?;
            // Bound before recursive Serde conversion; arbitrary Raw records fail.
            validate(&value, &self.state.limits.envelope())
                .map_err(|e| Error::resource_limit(e.to_string()))?;
            let request: Request = from_value(value)?;
            let id = self.state.command(
                &self.base,
                &self.principal(context),
                self.writable,
                context,
                request,
            )?;
            return Ok(Response::Written(Path::from_components(vec![
                "outstanding".to_string(),
                id,
            ])));
        }
        if p.len() == 3 && &p[0] == "outstanding" && &p[2] == "release" {
            let mut inner = self.state.inner.lock().unwrap_or_else(|e| e.into_inner());
            State::release(&mut inner, &p[1], &self.principal(context), self.writable)?;
            drop(inner);
            self.state.gate.notify();
            return Ok(Response::Written(p.slice(0, 2)));
        }
        Err(Error::permission_denied(
            "state data is read only; use operations",
        ))
    }
}

impl Service for View {
    fn call(
        &self,
        context: CallContext,
        operation: Operation,
    ) -> structfs_core_store::DetachedFuture<Response> {
        let this = Self::new(
            self.state.clone(),
            self.id,
            self.base.clone(),
            self.writable,
        );
        Box::pin(async move {
            match operation {
                Operation::Read(p) => this.read(&context, &p).await.map(Response::Read),
                Operation::Write(p, r) => this.write(&context, p, r),
                _ => Err(Error::invalid_argument("unsupported operation kind")),
            }
        })
    }
}
