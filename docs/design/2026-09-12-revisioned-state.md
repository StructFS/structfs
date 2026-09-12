# Revisioned state: P0-D implementation

Date: 2026-09-12

This implements P0-D from the [application substrate design](2026-09-11-application-substrate-and-value-ir.md).
The new `structfs-state` crate provides a portable v1 protocol, an in-memory
reference service, native async clients, and immutable projections implementing
StructFS `Reader`. Featherweight's `state` guest feature provides the same typed
protocol through its core-Wasm ABI. The `structfs` facade exposes a `state` feature.

## Consistency and authority

Create `State` under a retained service owner, then mount `state.view(base,
writable)` as a native service or a Featherweight import. Each view defines the
state subtree that its command payloads can address. Payload paths are validated
relative paths and are joined to this base. A readonly view cannot commit a batch.
A router client scoped to `data` grants reads only; granting write access to
`operations` grants command authority for the entire mounted state view. Use a
restricted view, not untrusted command fields, to establish a tenant boundary.

One provider mutex serializes state, revision, committed history, and handle
publication. This is one local consistency boundary; it does not cover unrelated
providers. Tokens contain a random incarnation UUID and a monotonic u64 revision.
A different epoch requires resynchronization. Provider revision is global across
its views, so unrelated updates can force a conditional batch to revalidate.

A batch checks its expected token, mutation count, paths, tree bounds, atomic
change-record size, and result-handle admission before publication. It applies
mutations in order to a private candidate. Any failure leaves state, revision,
and history unchanged. Every accepted nonempty batch increments revision, even
if values compare equal or deleting an absent node makes no net change. Overflow
fails before publication. Empty batches are rejected. Each intermediate candidate
must fit the bounds too; a temporary oversized tree is not admitted simply because
later mutations might shrink it.

Set replaces a subtree. Null is present data; deleting the root makes it absent.
Array set/remove semantics match Core Value, including index shifts on deletion.
History records are **committed invalidations**, not value diffs: they carry the
commit token and conservatively invalidate the parents of mutation paths. This
covers array shifts and repeated command occurrences. Views project those paths
to their own subtree and do not expose other tenants' path names or values.

The reference store's durability is memory-only. Atomic batches and observation
are semantics of this store protocol, not guarantees supplied by StructFS core.
Other stores may implement the protocol with durable write acknowledgments and
recovery guarantees of their own. A lost batch response can mean an already committed
write. Neither native nor guest clients retry batches automatically. Cancellation
after publication does not roll back effects. For application effects, read a
generation from a snapshot and commit its result using that snapshot's expected
token; a conflict forces revalidation before effect publication. Durable outboxes,
idempotency receipts and crash recovery require stores implementing those contracts;
applications select and compose them. The runtime does not provide these guarantees.

## Wire surface

| Path | Read/write contract |
|---|---|
| grant root | Read protocol version, durability, atomic record and page bounds |
| `data/{path}` | Read current Value or absence; writes are denied |
| `operations` | Write `{version:1, op, ...}`; returns an opaque `outstanding/{id}` path |
| `outstanding/{id}` | Read descriptor or typed operation fault |
| `outstanding/{id}/snapshot/{offset}` | Read pinned preorder node page |
| `outstanding/{id}/changes/{revision}` | Read/park for a bounded change page |
| `outstanding/{id}/release` | Write to release the handle; idempotent |

Commands are `batch {expected?, mutations}`, `snapshot {prefix, limits}`,
`observe {prefix, limits}`, and `watch {prefix, after, limits}`. Mutations are
`set {path,value}` and `delete {path}`. Page limits specify bytes and item count.
Read the root capabilities to negotiate them; providers reject incompatible
limits at handle creation.

Descriptors include a token plus `snapshot`, `watch`, and `committed` flags.
`observe` uses a single composite handle exposing both snapshot and change pages;
it does not allocate separately published handles. This makes reservation and
publication atomic without a partially delivered handle pair.

Handle reads use `{status:"ok",value:...}` or `{status:"error",value:fault}`.
Fault kinds include conflict (with current token), epoch mismatch, cursor expiry
(with earliest resumable token), invalid request, resource limit, and closed.
State faults therefore survive the diagnostic-only core-Wasm ABI as typed Values.
Transport, malformed envelope, authorization-surface, and admission errors remain
ordinary store errors. A faulted command can still consume a receipt handle until
release, owner cleanup, or age expiry. Native/guest clients release fault receipts.

Opaque handle IDs are bearer capabilities within their originating state view.
A handle created through one view is not addressable through a different view.
They are not globally routable paths. Returned paths remain subject to the shared
router's normal grant and client-scope confinement.

## Snapshots and observation

Snapshot construction and watch starting position share the commit mutex.
`observe` therefore pins initial state at revision R and watches commits after R
without a fetch/subscribe race. A standalone watch validates its supplied epoch
and revision against retained history before publication.

Snapshots use preorder nodes with raw string-component vectors. Containers carry
empty map/array skeletons; leaves carry their actual Values. This preserves empty
containers, Null, unsigned integers, bytes, and map keys that are not valid path
identifiers. Such keys are accessible by snapshot/value traversal or replacing an
addressable ancestor; the service does not invent an implicit key encoding.
Each snapshot page returns its fixed token, nodes, next offset, and `done`.
An empty snapshot means absence, not Null. Invalid offsets fail instead of clamping.

`StateHandle::projection` fetches under explicit client byte/node bounds, verifies
page tokens/cursors, and produces an immutable synchronous `Reader`. Renderers
can read this projection without initiating network or consuming event reads.
The projection is client-owned memory; it remains usable after server handle
release. Consumer copies and transport buffers require their own budgets.

Watch pages contain whole atomic commit invalidations with their tokens. Pages
never split one record. The next token advances over filtered-out commits, even
when no items match, preventing repeated scans of irrelevant history. A read at
the current revision parks until publication, cancellation, or handle expiry.
Future revisions and cursors preceding the watch's grant are invalid. Old history
returns `CursorExpired` with the earliest resumable token; it never jumps a stale
cursor silently to the current tail. Live watch pages have `done:false`; closing
or expiring their handle produces a typed closed fault rather than a final drain.

## Bounds and lifecycle

The provider bounds request bytes, state bytes/depth/nodes, batch mutations, history bytes/count,
atomic change-record bytes, snapshot bytes, aggregate handle storage, handle count,
handle age, and page bytes/items. Byte units are canonical Value JSON encoded
size, including envelopes for page checks, rather than allocator RSS. They apply
regardless of the selected transport codec. Codec limits remain an additional
bound. Command-envelope nesting has a separate allowance so it does not reduce
the permitted state-tree depth. Minimum page capacity is the maximum atomic record plus a fixed envelope
allowance; snapshot creation also checks each individual node fits a page.

History evicts the oldest complete records when full. Slow readers cannot pin it
or block writers indefinitely. A snapshot remains pinned even if its paired watch
expires from history; the caller must restart observation to restore continuity.
Oversized snapshots fail instead of producing a partial view. Flattening checks
path and node size incrementally, bounding expansion from repeated long paths.

The service owner reserves two state capacities (live and candidate), history
capacity, and one snapshot construction capacity upfront. The mutex bounds
concurrent construction workspace. Before publication, each handle reserves its
retained nodes and metadata with the initiating request/instance owner, falling
back to the service owner for unowned native calls. Bind native clients with
`Client::owned_by`; Featherweight wired calls already supply their instance owner.
The aggregate handle storage limit includes receipt/watch metadata too.

Release and owner cleanup remove handles and wake readers. Provider-owner cleanup
clears state, history, and every handle. Age expiry rejects access immediately;
parked watches use an expiry timer. Idle expired handles are reclaimed on the next
handle read, command, or owner cleanup. Thus memory is capacity-bounded even without
a background reaper, but the age bound is an accessibility bound, not a promise
of eager allocator reclamation at that exact instant. Native handle Drop itself
does not perform routed I/O; explicit release, owner close, and expiry are the
release mechanisms.

## Validation and remaining release work

Native tests cover all-or-nothing batches, concurrent expected-token conflicts,
overflow, Null/absence, no-op occurrences, pinned paged snapshots, history expiry,
epoch/future cursors, filtered advancement, page-size rejection, subtree grants,
request admission/cleanup, age expiry, and immutable projections. Portable
protocol tests preserve bytes, u64 extrema, and typed expiry through Value JSON,
CBOR, and FlexBuffers.

An actual Wasm guest observes, reads its snapshot, commits u64::MAX, and reads
changes through an import. Its abandoned handles exhaust the configured capacity
until instance shutdown releases them. The guest's typed `state` SDK also builds
for wasm32-unknown-unknown without the native provider/runtime dependencies.
The archive consumer exercises projections and rejection of a superseded effect.

Final validation: 1,322 workspace tests passed with all features; strict workspace
Clippy and formatting checks passed. The archive gate passed 645 tests, consumer
Clippy, guest Wasm feature builds, independent value-codec checks, and rustdoc for
the runtime, handles, service, and state crates. Protocol-only tests also passed.
No crates were published.

P0-E is next: the broader Ox/Horns-style external application fixtures, followed
by the coordinated packaging/release gates. This provider does not implement a
renderer, durable workflow engine, distributed transactions, or a production
network/process adapter.
