# Owned services: P0-C implementation

Date: 2026-09-11

This implements the ownership, registration, supervision, and bounded retention
priority in the [application substrate design](2026-09-11-application-substrate-and-value-ir.md).
It builds on the [shared service router](2026-09-11-shared-async-services.md).

## Ownership contract

`CleanupSupervisor` is retained by the embedding host through shutdown. It bounds
the number of owner records and retains unfinished resources even after the
original owner is dropped. A Tokio runtime must remain alive to execute cleanup.
`with_handle` supports hosts constructing their service substrate outside an
entered runtime.

`Owner` is a unique scope lifetime. `OwnerHandle` is cloneable admission authority;
keeping a handle does not keep the scope open after the unique owner is dropped.
Drop signals cancellation without waiting or executing user cleanup inline.
`cancel` closes admission permanently. `close(timeout)` initiates cancellation,
waits at most the supplied duration, and returns a `CloseReport`. Dropping a close
future does not stop cleanup. Repeated close observes the same retained resources.

Each owner limits resource count and retained byte reservations. Every resource
has a unique opaque `ResourceId`, kind, and byte charge. Names and mount prefixes
are not identities. Limits are immutable for this lifetime; existing live call
budget updates remain supported independently. Child owners consume a parent
registration and close when the parent closes. Their individual limits are
explicit; they do not silently inherit or replace an ancestor's call budget.
Admission checks ancestor cancellation even before child cleanup is scheduled.
Independent owners are unaffected.

A report distinguishes cancellation (`closed`) from completion (`is_quiescent`).
Remaining resources stay visible and charged. Task/cleanup panics and errors are
counted and leave a failed resource in the report; they are not successful
release. After reconciliation, the host can use `acknowledge_failure` on the owner
handle or retained supervisor. That operation is restricted to completed failures
and neither retries an irreversible operation nor asserts rollback. Reports carry
bounded resource metadata and failure counts, not unbounded error strings.
Completed owner records may be reclaimed when creating another owner.

## Provider and request work

`OwnerHandle::spawn` reserves a task slot before exposing work to the executor.
It supplies the owner's cancellation token and observes both errors and panics.
The task must cooperate or remain reported until it finishes. The API does not
return an abort handle whose accidental drop could detach the task from tracking.
Monitor tasks observe completion before releasing the slot. Noninterruptible work
must retain its context/lease and remain inside supervised work.

`owner.service(provider)` adds a lifetime to a native provider. `Client::owned_by`
adds request or operation lifetimes to a client. Cloning, scoping, or replacing
call metadata cannot remove those constraints. Closing a request client does not
revoke an independently owned service registration. Each owned provider boundary
reserves a resource slot; the existing routed call admission is shared through
its lease, without reacquiring call quota at that boundary.

`CallContext::owner()` gives providers their initiating owner. An outer request
owner is preserved through nested service wrappers; otherwise the mounted
provider's owner is used. Providers register newly opened handles with this owner
before delivering them. Work that must survive a request must instead explicitly
use a separately retained service owner at creation time. Raw `tokio::spawn` or
provider-created threads are not discovered automatically; using them without
retaining accounting and supervision violates the provider contract.

An owned call checks admission and cancellation before dispatch. Once dispatched,
cancellation abandons the result and signals the provider's operation token.
Running blocking work keeps both the call admission lease and ownership
reservation. A successful close therefore cannot mistake caller abandonment for
provider completion. Cancellation cannot undo committed effects.

## Registrations and cleanup authority

`OwnerHandle::open` reserves the cleanup slot and byte budget before dispatching
an asynchronous handle open. The opener returns a public value and a host release
callback. Once dispatched, opening is observed to completion even if the caller
abandons delivery. Cleanup joins the opener and then releases its handle; a lost
result never transfers cleanup responsibility to the vanished caller. The
delivered `OwnedResource` retains its registration and rejects access after
release. An opener returning an error must have released its partial resources;
a panic is reported as an uncertain outcome requiring reconciliation.

`OwnerHandle::register` reserves a slot and optional bytes before a provider handle
can be exposed. The returned unique `Registration` owns an exactly-once cleanup
callback. Dropping or releasing it signals revocation immediately and schedules
cleanup, independently of the expired request's cancellation/deadline. Owner
cancellation does the same for all registrations. Cleanup callbacks are constructed
and executed inside observed tasks, including panic handling.

The registration's existing slot is the cleanup reservation: full ordinary
admission cannot prevent its cleanup from being scheduled. At most a worker and
its monitor are created per accepted task/cleanup entry. The host must supply
capability-limited cleanup authority in the callback. If release requires routed
I/O, use an explicitly provisioned host cleanup client and budget; a revoked
request client cannot be turned back into cleanup authority.

`Router::register(owner, mount)` installs an owned dynamic mount. The router
serializes installation/removal, rejects duplicate prefixes, and gives each
registration a new identity. Release immediately rejects new calls and signals
in-flight calls even if physical route removal has not yet run. Callers can await
`Registration::close` before registering a replacement at the same prefix. Old
registration handles cannot revoke the replacement. Existing cloned/scoped
clients observe revocation. Static constructor mounts remain available for
application-lifetime configuration.

## Retained results and tails

`RetainedBytes` registers its storage before delivery and charges the vector's
capacity. Losing result delivery drops the registration and starts cleanup.
Owner close clears storage even if the receiver retains the public handle.
Snapshots after release fail. Copies returned to consumers are consumer-owned
buffers and require separate response/transport accounting.

`OwnedTail` reserves its declared payload capacity and logical item metadata
capacity upfront. It bounds both item count and retained vector capacity. Full
writes return `Overloaded`; there is no silent eviction. `acknowledge(cursor)`
explicitly discards older entries and recovers write capacity. Stale and future
cursors return conflict errors. Sequence overflow is rejected.

Reads have an explicit item page bound and return items, the next cursor, and
terminal status atomically. `done` becomes true only when that page reaches the
finished tail's end. Finish preserves readable entries; release/owner close
clears them and wakes parked readers. Cancellation also wakes a parked read.
Limits describe retained payload capacity and logical metadata, not allocator
RSS. The older unbounded `structfs-handles::TailLog` remains a compatibility API;
new application operation/result surfaces should use the bounded owned API.

## Featherweight integration

Every assembly receives a provider owner from the runtime's retained supervisor.
Its wired namespace dispatch passes through owned services, covering both host
providers and mailbox routing. Providers can use `CallContext::owner()` for
handle cleanup and background work. `AssemblyInstance::provider_owner()` exposes
the same host-side authority.

Graceful shutdown allows guest cleanup during the grace period. Before joining
remaining execution tasks, the runtime closes provider owners across the instance
tree. `ShutdownReport::providers` reports remaining owned work and cleanup;
`complete()` now requires both guest joins and provider quiescence. A guest can
be stopped and removed from the block registry while its noninterruptible native
operation remains reported and charged. Hosts can retain
`Runtime::cleanup_supervisor()` even after dropping runtime/instance handles.

The runtime bounds retained owner records at 65,536. Default per-instance provider
limits are 65,536 resource entries and 16 MiB retained bytes;
`with_provider_limits` configures subsequent instances. Routed call budgets remain
independent. Native `OwnerLimits::default()` uses 1,024 entries and 16 MiB.

A persistent guest's internal provider operations belong to its instance. Existing
`AssemblyRequest` cancellation still removes that request's pending calls without
claiming attribution of all guest work to it. Native request clients use explicit
`Client::owned_by`; attributing guest-created effects to individual request
messages requires an application protocol and is not inferred from task timing.
The reserved `iso` control surface retains its runtime lifecycle implementation.

## Verification and next priority

Independent native fixtures cover drop and bounded close, resource saturation,
child cancellation, unrelated request isolation, cleanup failure reconciliation,
abandoned close futures and result delivery, admission before handle creation,
noninterruptible handle opens, dynamic mount revocation/replacement,
blocking work retaining admission, and tail capacity/cursors/terminal reads.

An actual Wasm guest fixture completes execution while a provider operation stays
running. Instance shutdown reports that operation; a host-retained supervisor joins
it after instance/runtime drop and observes the final admission release. The
archive consumer exercises owned mounts, scoped clients, and bounded tails using
only packaged crates.

Validation on the completed implementation: 1,309 workspace tests passed with
all features; workspace Clippy passed with warnings denied. The archive gate
passed 631 tests plus external-consumer Clippy, the independent value encoder,
guest Wasm builds, and documentation checks. Formatting and diff checks passed.
No crates were published.

P0-D is next: revisioned state, atomic updates, and bounded observation. This step
does not implement application-specific subprocess/network cleanup, durable
operation recovery, or automatic migration of legacy unbounded logs.
