# Shared async services: P0-B implementation

Date: 2026-09-11

Scope: `structfs-service`, Featherweight wired namespaces, shared call admission

This implements P0-B from the
[application-substrate priorities](2026-09-11-application-substrate-and-value-ir.md).
The native service layer does not depend on Featherweight or Wasmtime. Both native
clients and Featherweight wired calls now use its routing and admission machinery.

## Verified starting point

Core already exposed detached read/write traits and cloneable synchronous stores.
Featherweight had detached host-provider dispatch, namespace prefix mapping,
delegated grant rewriting, and hierarchical mailbox admission. However, direct
providers bypassed that admission, and application hosts could not reuse the
namespace implementation without depending on the runtime.

P0-A established Value v1 before this work. A full assembly test added here found
and fixed one remaining integration gap: the runtime manifest allowlist did not
yet accept `application/vnd.structfs.value+json;version=1`, although the codecs
and lower-level Wasm execution path already supported it.

## Native contracts

`Service::call(context, operation)` constructs a detached, `Send + 'static` future
promptly. `Operation` is either a read or write with a provider-relative path;
`Response` is a read result or returned write path. Providers must return the
corresponding response kind. No `WasmBlockDriver` is involved.

`Mount` specifies a caller-visible prefix, provider base, read/write permissions,
service, and admission policy. `Router::new` rejects duplicate mounts and returns
a shared router. Its cloneable `Client` implements the core async and
detached store traits as well as inherent concurrent `read` and `write` methods.

`Client::scoped` appends a validated component path and intersects permissions.
It cannot restore a permission removed by an earlier scope. Most-specific mounts
shadow broader ones even when their permissions deny the operation; there is no
fallback through a broader grant. Unwired paths fail with permission denied.

Paths map in this order:

1. Join the client's base and supplied relative path.
2. Resolve the longest matching component prefix.
3. Replace that prefix with the provider base.
4. Dispatch to the provider under the mount's admission policy.
5. For writes, remove the provider base from the returned path, restore the mount
   prefix, and remove the client base.

Either failed removal in step 5 is a permission error. A returned path cannot
escape the provider grant or client scope. This validation does not undo effects
the provider may already have committed.

`RouteTable<T>` supplies the common prefix implementation used by the router and
Featherweight wiring. Its lower-level compatibility constructor preserves the
first entry for equal prefixes; the new `Router` constructor rejects duplicates.

## Context and cancellation

`CallContext` carries a request identifier, cancellation token, deadline, and a
private admission lease. Request identifiers are diagnostic metadata, not
credentials or deterministic application values. Authority comes from the client
and mounts. `with_context` lets a host propagate a request's cancellation and
absolute deadline across calls; `with_timeout` checks deadline representability.

The router checks cancellation/deadline before admission and again before
dispatch. Each dispatched operation receives its own cancellation token. Caller
abandonment, cancellation, deadline expiry, or completion signals that token.
Cancellation drops the returned provider future; it does not claim rollback.

Providers can inspect `ensure_active()` before committing work and retain
`context.lease()` across noninterruptible work. The router independently retains
its reservation even if a provider ignores or immediately drops its context.
Custom providers remain responsible for any work they start outside their
returned future.

Tracing uses a `structfs.call` span carrying request identifier, mount, and
operation. Service implementation details do not become wire fields. The native
Rust API requires Send/Sync; those constraints are not additional wire-schema
requirements for a browser implementation.

## Provider adapters

| Adapter | Execution and cancellation contract |
|---|---|
| `DetachedProvider<T>` | Holds the dispatch mutex only while constructing the future; releases it before waiting |
| `ImmediateStore<T>` | Explicit opt-in to short synchronous operations on the caller thread |
| `BlockingStore<T>` | Dispatches synchronous work to Tokio's blocking executor; checks cancellation before entering the operation and retains its lease until work finishes |

`BlockingStore::close()` atomically stops new provider admissions and waits for
its accepted blocking work. `active()` reports remaining work. Dropping a caller
does not detach that work from accounting, and dropping a close future does not
reopen the provider. A noncooperative running operation can prevent close from
finishing. The host must retain the adapter and decide how to supervise it.

Main-thread-affine objects can stay behind an application-owned message adapter
implementing `Service`; the router does not need to own a DOM or terminal object.
A platform-specific UI/process adapter is not included in P0-B.

## Admission

The previously Featherweight-specific `CallBudget` implementation now lives in
`structfs-service` and is generic over an admission key. Featherweight re-exports
its existing `CallBudget<BlockId>` API. The `calls_per_block` and
`bytes_per_block` field names retain compatibility; the native API interprets
them as per-key limits. Applications choose keys when configuring a mount's
`BudgetAdmission`; callers do not choose them per operation.

Hierarchies are immutable; each accepted call reserves at every configured budget
level. Live policy changes retain existing charges and affect later admissions.
Reads, parsed writes, and raw writes are admitted. Byte accounting represents
logical retained request weight, not RSS, response size, or provider buffers.

`Lease` shares one reservation rather than acquiring a second one when cloned.
Its last owner releases the charge. A cancelled blocking call can therefore
continue consuming its slot until the blocking closure exits. Failed admission
dispatches no provider work.

## Featherweight integration

Wired namespace reads and writes use a shared `Client`. The runtime's reserved
`iso` surface, root listings, transcript interception, session witnessing, and
simulation scheduling remain runtime responsibilities around that boundary.
Execution-scope cancellation and deadlines are propagated into service context.

Delegated grants are flattened into a final target and provider base before
dispatch. Prefix mapping and returned-path confinement are performed by the
shared router. The legacy grant-store facade also uses the shared router.

Mailbox targets transfer the router's existing lease into their pending-call
correlation. They do not reacquire call admission. Other existing direct runtime
mailbox entry points still acquire a lease themselves. Mailbox event/reply
reservations remain separate resources and are not suppressed.

Direct host-provider calls now consume the runtime's call budget. Native mount
partitions receive opaque admission keys; mailbox partitions retain their existing
cell admission keys. Global and ancestor limits apply across both target kinds.
Applications should review existing budgets because workloads previously bypassing
them can now return Overloaded. Root/`iso` control operations are not routed
provider admissions, preserving the existing shutdown/control path.

A canceled mailbox request can return before its server observes cancellation
and drops a pending native-provider read. Its request budget is released while
the independent provider charge remains until that read ends. Global quiescence
requires provider completion or instance shutdown, rather than caller return.

`service_host_store(Arc<dyn Service>)` registers a native service as an assembly
import. Existing sync/detached host-store constructors remain supported. Running
legacy synchronous provider work retains the lease inside the blocking closure;
its result may be abandoned, but its charge survives until completion.

## Verification and remaining priorities

Native tests cover component boundaries, longest-prefix shadowing, read-only
attenuation, grant and client return-path escape, request-context propagation,
concurrent detached dispatch, pre-dispatch rejection, cancellation/deadlines,
caller abandonment, budget ancestry and updates, raw payload admission, and
blocking work retaining its reservation until close can finish.

New actual-guest tests use tagged JSON and maximum u64 values through native
imports. A single-slot budget proves sequential import calls are admitted once;
a zero-slot budget proves rejection happens before provider dispatch. A separate
guest-to-mailbox fixture proves the existing reservation is transferred without
double charging. Existing grant, overload, deterministic transcript, and capacity
fixtures exercise the runtime integration.

The archive gate includes the new service crate tests and an external native
router consumer. It still checks the value corpus and guest codec builds. Run:

```sh
cargo test -p structfs-service -p featherweight-runtime --offline
cargo test --workspace --all-features --locked --offline
cargo clippy --workspace --all-targets --all-features --locked --offline -- -D warnings
scripts/check-featherweight-release.sh
```

P0-C is now implemented in [owned services](2026-09-11-owned-services.md), including
owned dynamic mounts, provider supervision, explicit cleanup, and bounded byte
results/tails. Revisioned state and observation remain P0-D.
