//! Process control: spawn/wait/kill as the handle pattern
//! (`isotope/spec/09-posix-closure.md`).
//!
//! One protocol serves two surfaces:
//!
//! - `iso/proc` for blocks whose definition grants `spawn: true`;
//! - [`crate::Runtime::management_store`], the runtime's own management
//!   surface (spec 08's "management API is StructFS").
//!
//! Writing an assembly definition (as a Value) mints `outstanding/{id}`.
//! Reading the handle returns `{name, state, code}`; reading
//! `outstanding/{id}/wait` parks until the assembly's public block is
//! terminal — `wait(2)` is a blocking read. A Null write shuts the
//! assembly down and releases the handle — `kill(2)` is the release.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use structfs_core_store::{DetachedFuture, Error, Path, Record, Value};
use structfs_handles::{CancelToken, HandleCx, HandleProtocol, HandleStore};

use crate::assembly::AssemblyDef;
use crate::runtime::{AssemblyInstance, RuntimeInner};

/// A spawn/management store: `HandleStore` over [`SpawnProtocol`].
pub type ProcStore = HandleStore<SpawnProtocol>;

/// The handle protocol for spawned assemblies.
pub struct SpawnProtocol {
    runtime: Weak<RuntimeInner>,
    base_dir: PathBuf,
    handle: tokio::runtime::Handle,
}

impl SpawnProtocol {
    pub(crate) fn store(
        runtime: Weak<RuntimeInner>,
        base_dir: PathBuf,
        handle: tokio::runtime::Handle,
    ) -> ProcStore {
        HandleStore::new(Self {
            runtime,
            base_dir,
            handle,
        })
    }
}

/// Per-handle state: one spawned assembly.
pub struct SpawnedAssembly {
    instance: Arc<AssemblyInstance>,
    cancel: CancelToken,
}

impl HandleProtocol for SpawnProtocol {
    type Handle = SpawnedAssembly;

    fn open(&self, cx: HandleCx, request: Value) -> Result<Self::Handle, Error> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| Error::overloaded("runtime is shutting down"))?;
        let def = AssemblyDef::from_value(&request)
            .map_err(|e| Error::store("proc", "spawn", e.to_string()))?;
        let instance = runtime
            .instantiate(&def, HashMap::new(), &self.base_dir)
            .map_err(|e| Error::store("proc", "spawn", e.to_string()))?;
        Ok(SpawnedAssembly {
            instance,
            cancel: cx.cancel,
        })
    }

    fn read(&self, handle: Arc<Self::Handle>, sub: Path) -> DetachedFuture<Option<Record>> {
        Box::pin(async move {
            if sub.is_empty() {
                // Status: {name, state, code}.
                return Ok(Some(Record::parsed(
                    handle.instance.public_cell().status_value(),
                )));
            }
            if sub.len() == 1 && sub[0] == "wait" {
                // wait(2): park until terminal; released handles interrupt.
                let public = handle.instance.public_cell().clone();
                tokio::select! {
                    _ = public.wait_terminal() => {}
                    _ = handle.cancel.cancelled() => {
                        return Err(Error::cancelled("spawn handle released"));
                    }
                }
                return Ok(Some(Record::parsed(public.status_value())));
            }
            Ok(None)
        })
    }

    fn write(&self, _handle: Arc<Self::Handle>, sub: Path, _data: Record) -> DetachedFuture<Path> {
        Box::pin(async move {
            Err(Error::store(
                "proc",
                "write",
                format!("spawn handles have no writable sub-path: {}", sub),
            ))
        })
    }

    fn close(&self, handle: Arc<Self::Handle>) {
        // kill(2): release triggers shutdown. Request graceful shutdown
        // synchronously — parked blocks unblock even if the runtime is
        // torn down before the escalation task runs — then escalate
        // off-thread (close itself is synchronous).
        handle.instance.request_shutdown();
        let instance = handle.instance.clone();
        self.handle.spawn(async move {
            instance.shutdown(Duration::from_secs(5)).await;
        });
    }
}
