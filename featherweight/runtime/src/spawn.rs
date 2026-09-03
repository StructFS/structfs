//! Process control: spawn/wait/kill as the handle pattern
//! (`isotope/spec/09-posix-closure.md`).
//!
//! One protocol serves two surfaces:
//!
//! - `iso/proc` for blocks whose definition grants `spawn: true`;
//! - [`crate::Runtime::management_store`], the runtime's own management
//!   surface (spec 08's "management API is StructFS").
//!
//! Writing a definition (or `{definition, grants}`) mints
//! `outstanding/{id}`. Reading the handle returns `{name, state, code}`;
//! reading `outstanding/{id}/wait` parks until the assembly's public
//! block is terminal — `wait(2)` is a blocking read. Operations on
//! `outstanding/{id}/store/...` route to the child's public store — the
//! handle is the parent's channel to its child. A Null write shuts the
//! assembly down and releases the handle — `kill(2)` is the release.
//!
//! # Grants
//!
//! A spawner may bind the child's declared imports to slices of its own
//! namespace: `{"definition": {...}, "grants": {"logger": "services/logs"}}`.
//! Each grant path must resolve inside the spawner's wiring — a block can
//! only delegate capabilities it holds, so grants attenuate, never widen.
//! The management surface has no namespace and accepts no grants.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use structfs_core_store::{DetachedFuture, Error, Path, Record, Value};
use structfs_handles::{CancelToken, HandleCx, HandleProtocol, HandleStore};

use crate::assembly::AssemblyDef;
use crate::namespace::{host_store, GrantStore, HostStore, WiringTable};
use crate::runtime::{AssemblyInstance, RuntimeInner};

/// A spawn/management store: `HandleStore` over [`SpawnProtocol`].
pub type ProcStore = HandleStore<SpawnProtocol>;

/// The handle protocol for spawned assemblies.
pub struct SpawnProtocol {
    runtime: Weak<RuntimeInner>,
    base_dir: PathBuf,
    handle: tokio::runtime::Handle,
    /// The spawner's wiring, for resolving grants. `None` for the
    /// management surface, which has no namespace to delegate from.
    spawner_wiring: Option<Arc<WiringTable>>,
}

impl SpawnProtocol {
    pub(crate) fn store(
        runtime: Weak<RuntimeInner>,
        base_dir: PathBuf,
        handle: tokio::runtime::Handle,
        spawner_wiring: Option<Arc<WiringTable>>,
    ) -> ProcStore {
        HandleStore::new(Self {
            runtime,
            base_dir,
            handle,
            spawner_wiring,
        })
    }

    /// Split a spawn request into its definition and grant bindings.
    fn parse_request(request: &Value) -> (Value, BTreeMap<String, String>) {
        if let Value::Map(map) = request {
            if let Some(definition) = map.get("definition") {
                let mut grants = BTreeMap::new();
                if let Some(Value::Map(entries)) = map.get("grants") {
                    for (name, path) in entries {
                        if let Value::String(path) = path {
                            grants.insert(name.clone(), path.clone());
                        }
                    }
                }
                return (definition.clone(), grants);
            }
        }
        // Bare definition, no grants.
        (request.clone(), BTreeMap::new())
    }

    fn resolve_grants(
        &self,
        runtime: &Arc<RuntimeInner>,
        grants: BTreeMap<String, String>,
    ) -> Result<HashMap<String, HostStore>, Error> {
        if grants.is_empty() {
            return Ok(HashMap::new());
        }
        let Some(wiring) = &self.spawner_wiring else {
            return Err(Error::permission_denied(
                "grants require a spawner namespace; the management surface has none",
            ));
        };
        let mut imports = HashMap::new();
        for (name, raw_path) in grants {
            let path = Path::parse(&raw_path)
                .map_err(|e| Error::store("proc", "spawn", format!("bad grant path: {e}")))?;
            // A block can only delegate what it holds: the grant must
            // resolve inside its own wired namespace.
            let Some((target, rel, _prefix)) = wiring.resolve(&path) else {
                return Err(Error::permission_denied(format!(
                    "cannot grant '{raw_path}': path is not wired into the spawner's namespace"
                )));
            };
            imports.insert(
                name,
                host_store(GrantStore::new(runtime.ctx(), target.clone(), rel)),
            );
        }
        Ok(imports)
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
        let (definition, grants) = Self::parse_request(&request);
        let def = AssemblyDef::from_value(&definition)
            .map_err(|e| Error::store("proc", "spawn", e.to_string()))?;
        let imports = self.resolve_grants(&runtime, grants)?;
        let instance = runtime
            .instantiate(&def, imports, &self.base_dir)
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
            if sub[0] == "store" {
                // The parent's channel to its child: route to the
                // child's public store.
                let rel = sub.slice(1, sub.len());
                return handle
                    .instance
                    .read(rel)
                    .await
                    .map(|v| v.map(Record::parsed));
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

    fn write(&self, handle: Arc<Self::Handle>, sub: Path, data: Record) -> DetachedFuture<Path> {
        Box::pin(async move {
            if !sub.is_empty() && sub[0] == "store" {
                let rel = sub.slice(1, sub.len());
                let value = data.into_value(&structfs_core_store::NoCodec)?;
                let result = handle.instance.write(rel, value).await?;
                // Result paths come back in the handle's namespace.
                return Ok(Path::parse("store").unwrap().join(&result));
            }
            Err(Error::store(
                "proc",
                "write",
                format!("spawn handles accept writes only under 'store/': {}", sub),
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
