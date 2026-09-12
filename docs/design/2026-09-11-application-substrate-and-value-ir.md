# StructFS value IR and application substrate

Date: 2026-09-11

Status: Proposed design for the next coordinated crate release

Scope: StructFS core and service libraries, Isotope contracts, Featherweight embedding

## 1. Purpose and release outcome

Provide a supported foundation for reactive interfaces, streaming gateways,
agent execution, and persistent conversation services. Ox and Horns are the
reference consumers. They are willing to migrate to new contracts; compatibility
with their current broker callbacks, Wasm imports, and configuration conventions
is not a design constraint.

The migration target is an application built from stores and explicitly owned
services. An application can combine native code and Wasm blocks without
reimplementing routing, cancellation, change observation, or resource accounting.
Its data has the same documented meaning across local calls and supported
encodings.

This is a design specification, not a claim that the proposed APIs exist or an
implementation task plan. Existing implementation evidence is separated in
section 3. Names for new Rust APIs and crates below are provisional; normative
behavior and acceptance requirements are the proposed contract.

The [StructFS Value v1 specification](../specs/structfs-value-v1.md) expands and
refines sections 5 and 6 below with normative codec rules, canonical JSON bytes,
typed conversion behavior, limits, and conformance vectors. It remains a draft
contract rather than a claim of implementation.

The [existing embedding release gate](../featherweight-release-readiness.md)
records completed runtime work. This proposal adds application-level release
requirements; it does not retroactively change those recorded results. Publication
requires a fresh gate run against the final implementation.

## 2. Decisions

1. Evolve `structfs_core_store::Value` into the supported semantic IR. Keep one
   core value model; do not introduce a competing general-purpose value enum.
2. Implement typed Serde conversion directly against that IR, with an explicit
   structural mapping and checked failures. Do not route it through JSON.
3. Distinguish plain external formats from versioned StructFS lossless encoding
   profiles. Serde support alone is not a fidelity guarantee.
4. Keep the core interface based on reads and writes. Enumeration, state,
   observation, and operations are optional service profiles carried over it.
5. Supply one async router/client/provider composition layer for native hosts and
   Featherweight. A native application must not require Wasmtime merely to use
   the application substrate.
6. Supply a revisioned state provider with snapshots, conditional atomic batches,
   and bounded change observation. Atomicity is within one provider.
7. Make ownership cover provider operations, handles, subscriptions, and tasks,
   as well as guest execution. Cancellation is not rollback.
8. Keep UI schemas, approval policy, conversation persistence, and distributed
   orchestration in applications. Provide the lower-level contracts they need.
9. Gate the release on external reactive-screen, streaming-gateway, and
   conversation-service fixtures built from actual Cargo archives.

## 3. Prerequisites and verified baseline

Source baselines: StructFS `fdb6b01d28cfdfa3134cd6efc1a4b18cb3aebf02`;
Ox `0af682d5ac225ec55c22a1d1121d3473c25716b7`. Commands are relative to the
StructFS repository; `../ox` is the sibling checkout. Line references describe
these revisions. Ox references are evidence, not build dependencies.

- [x] **B1. The existing core already has a semantic value and raw/parsed record
  split.** Verified with `rg -n 'pub enum Value|pub enum Record'
  packages/core-store/src/{value,record}.rs` and source reads:
  `value.rs:22`, `record.rs:39`. `Value` has i64 integers, f64 floats, bytes,
  arrays, and string-keyed maps. Recheck if those representations change; affected
  scope is the IR and codec migration, not the service architecture.
- [x] **B2. Typed Serde conversion currently passes through JSON.** Verified with
  `sed -n '1,80p' packages/serde-store/src/convert.rs`: `from_value:8`,
  `to_value:16`, `value_to_json:24`. Bytes become base64 strings; non-finite
  floats and unknown variants can become null. Recheck before replacing these
  helpers; affected scope is conversion compatibility.
- [x] **B3. Native CBOR/FlexBuffers codecs use Value's Serde implementation.**
  Verified with `sed -n '60,120p' packages/serde-store/src/codec.rs` and
  `sed -n '1,215p' packages/core-store/src/serde_impls.rs`. Generic Value
  deserialization uses `deserialize_any`; `Path` Serde uses a string. Recheck
  before choosing codec implementation paths; no guarantee of arbitrary-format
  support follows from these implementations.
- [x] **B4. Enumeration and async interfaces are asymmetric.** Verified with
  `rg -n 'read_children|pub trait Detached|pub struct SyncToAsync'
  packages/core-store/src/{traits,async_traits}.rs`: `traits.rs:70`,
  `async_traits.rs:129,199,207`. Reading the adapter shows synchronous work
  executes during future polling. Recheck before changing adapters or helpers;
  affected scope is the portable client surface.
- [x] **B5. The two PrefixSuffix patterns differ.** Verified with
  `sed -n '31,77p' packages/core-store/src/path_pattern.rs` and
  `sed -n '55,105p' ../ox/crates/horns-core/src/subscription.rs`. Core permits
  an empty middle; Horns requires at least one component. Migration must review
  patterns rather than mechanically replace their types.
- [x] **B6. Horns has live readers and broker-level inferred changes.** Verified
  with `rg -n 'snapshot.*live|pub struct SubCtx'
  ../ox/crates/horns-core/src/subscription.rs` (`145,157`) and
  `rg -n 'let before|let after|sub.handle|write_at_depth'
  ../ox/crates/ox-broker/src/dispatching_store.rs` (`145,151,184,214`). Recheck
  before migrating callbacks; affected scope is observation/consistency semantics.
- [x] **B7. Background effects can start before returned state writes land.**
  Verified with `sed -n '180,250p'
  ../ox/crates/ox-gate/src/subscriptions/catalog_refresh.rs`: Refreshing is
  accumulated at `191`, prior work aborted at `197`, new work spawned at `204`,
  and the write list returned afterward. This motivates an explicit effect
  ordering contract; it is not a claim of a reproduced production race.
- [x] **B8. Screen lifecycle uses descriptive subscription IDs.** Verified with
  `rg -n 'SubscriptionId\('
  ../ox/crates/horns-core/src/install.rs
  ../ox/crates/horns-ratatui/src/install.rs` (`322,330,343` and `68`), and
  `sed -n '265,286p' ../ox/crates/ox-broker/src/lib.rs`. Unregister removes
  all matching IDs. Recheck before migration; multi-instance lifecycle isolation
  remains a required fixture regardless of the current implementation.
- [x] **B9. UI rendering already benefits from materialized projections.**
  Verified with `sed -n '1,115p'
  ../ox/crates/ox-cli/src/settings/snapshot.rs`: the snapshot is populated
  asynchronously and includes key material even though the UI needs presence
  (`55,68,78`). `horns_loop.rs:37,110` owns and tears down a terminal session.
  Recheck projection boundaries before moving UI code across a capability boundary.
- [x] **B10. Featherweight already separates artifacts, instances, and requests.**
  Verified with `rg -n 'pub struct (AssemblyRequest|ShutdownReport)|pub fn
  register_artifact|pub fn request|pub async fn shutdown'
  featherweight/runtime/src/runtime.rs`: `645,680,723,827,1171`. The
  independent consumer is `tests/embedding/src/lib.rs`. Reuse these contracts;
  this proposal does not ask to rebuild that work.
- [x] **B11. Mailbox admission does not automatically charge direct providers.**
  Verified with `sed -n '310,328p' featherweight/runtime/src/runtime.rs` and
  `sed -n '320,394p' featherweight/runtime/src/namespace.rs`. The former
  acquires a call charge; direct `Target::Store` dispatch uses another path.
  Recheck before adding middleware to avoid double charging.
- [x] **B12. Some stream/SDK primitives need stronger application contracts.**
  Verified with `sed -n '51,155p' packages/handles/src/tail.rs`: an unbounded
  vector and all-available tail pages; `featherweight/guest/src/lib.rs:62,73`
  returns string errors. Bounded duplex streams already exist in
  `packages/handles/src/duplex.rs`. Scope is extending/reusing these primitives.
- [x] **B13. Ox cleanup and durability have independent owners.** Verified with
  `sed -n '56,99p' ../ox/crates/ox-gateway/src/broker_block.rs`: cancellation
  interrupts reads while writes remain available. `ox-kernel/src/log.rs:282`
  commits before publishing at `284`; `ox-executor/src/agents.rs:2123,2212`
  separates guest execution from configuration snapshots. Recheck before the
  conversation fixture; no runtime change may weaken these invariants.
- [x] **B14. Archive-based verification is implemented.** Verified with
  `sed -n '30,86p' scripts/check-featherweight-release.py`: actual archives are
  extracted and an independent consumer is built against them. Extend this gate
  rather than substituting workspace-only tests.

## 4. Layering and application domains

| Layer | Responsibility | Excluded responsibility |
|---|---|---|
| StructFS core | Value, Record, Path, errors, read/write traits, basic composition | UI types, task executor, application transactions |
| StructFS service libraries | Async routing/client handles, state/change protocol, bounded operations and streams | Wasm execution, application policy |
| Isotope | Block ABI, assembly/lifecycle/capability rules, service profiles | A universal database or UI model |
| Featherweight | Block execution, assembly composition, runtime admission and owned teardown | Conversation truth, credential policy, VM placement |
| Host adapters | Terminal, filesystem, HTTP, process and transport implementations | Implicit ambient authority for guests |
| Ox and Horns | Views, commands, approvals, conversation semantics and persistence policy | Duplicated generic routing and ownership machinery |

New service modules may ship as an optional `structfs-services` crate and an
executor integration crate, or equivalent narrowly separated crates. Final names
are an implementation decision. The dependency direction is normative: services
must not depend on Featherweight, Horns, Ratatui, or Ox. Featherweight uses the
shared service layer. Pure values and protocols must not require Tokio/Wasmtime.

| Domain | State owner | Work unit | Observation |
|---|---|---|---|
| Reactive screen | Screen/application state provider | Ordered input and owned background effect | Latest render projection plus reliable input acknowledgment |
| Gateway | Request and downstream operation owners | Fresh guest execution or explicit service request | Bounded streaming response |
| Conversation | Persistent conversation service | Fresh turn, tool operation, approval wait | Durable event cursor and current projection |
| Remote worker | Worker and conversation services | Idempotent application operation | Reconnectable bounded observation |

## 5. The value IR

### 5.1 Semantic model

The release retains the existing structural model and adds exact unsigned integer
support. The proposed set is:

```text
Null
Bool
Integer       mathematical integers in [-2^63, 2^64 - 1]
Float         IEEE 754 binary64
String        UTF-8
Bytes         uninterpreted bytes, distinct from String and Array
Array         ordered heterogeneous values
Map           unique UTF-8 string keys to values
```

Rust may retain `Integer(i64)` and add `Unsigned(u64)` for compatibility with the
current layout. Signedness is not a separate semantic type: equal nonnegative
integers compare equal. Normalization uses `Integer` when the value fits i64 and
`Unsigned` otherwise. Constructors, decoded values, and canonical output use
that normalization. Out-of-range i128/u128 conversions fail; arbitrary precision
is not part of this release.

Integers and floats remain distinct even when numerically equal. Map ordering
does not carry application meaning. Duplicate input map keys are rejected before
they can be overwritten during decoding. Map keys need not themselves be valid
Path components; path-based access follows the existing validated-path rules.
Data keys and externally supplied filenames must not silently undergo identifier
normalization. Named profiles must specify any reversible key encoding.

Null is a present value. Path absence remains `Ok(None)` outside the IR. Deletion
is an operation defined by a service, not a universal meaning of Null. Existing
Null-delete tree/handle conventions remain identifiable profiles; the new state
batch protocol has separate `set` and `delete` operations and can store Null.

### 5.2 Equality and normalization

Define an explicit semantic comparison used by state change detection and
conformance tests:

- Integer comparison is exact across signed/unsigned storage variants.
- Positive and negative floating zero are distinct.
- All NaN payloads/signs normalize to one semantic NaN.
- Other float values, including infinities, preserve their binary64 value.
- Arrays compare in order; maps compare by keys and recursively by value.
- Bytes never equal an array of numbers or a base64 string.

Do not use derived floating-point `PartialEq` as the state provider's change
predicate. Introduce `semantic_eq`/normalization helpers and document any change
to ordinary Rust equality separately. No stable hash is inferred from BTreeMap
ordering, Debug output, or a codec's default output. The value specification now
defines canonical tagged JSON bytes and conformance vectors. A content-hashing
protocol remains separate and deferred. Existing transcript digests retain their
declared version.

### 5.3 Value and Record

`Value` is the semantic IR. `Record::Raw { bytes, format }` retains original
encoded bytes and `Record::Parsed(Value)` permits semantic inspection. Parsing
does not promise to retain source whitespace, comments, duplicate spellings,
encoding widths, or format-specific metadata. Raw records remain forwardable
without parsing. Unknown/unsupported formats fail conversion explicitly.

Resource policy bounds decoding depth, collection sizes, total allocations,
individual strings/blobs, and encoded output. Apply limits while parsing and
serializing, before allocation where possible. A wire byte limit alone is not a
decoded-allocation limit. Diagnostic output must also be bounded.

## 6. Serde and encoding fidelity

### 6.1 Two different contracts

Implementing `Serialize`/`Deserialize` for Value enables a codec to process the
IR. Implementing a Serde `Serializer` and `Deserializer` over Value enables
direct conversion between typed Rust values and the IR. The latter must replace
the JSON intermediate in `to_value`/`from_value`.

Every supported typed conversion is specified relative to its destination type
and profile. It is not arbitrary Rust reflection. Custom Serde implementations
can change representations and cannot be reversed without their declared contract.
Only the serialization model exposed by those implementations is available.

### 6.2 Structural Serde mapping

The default direct bridge uses this mapping:

| Serde shape | Structural mapping |
|---|---|
| bool, string, char | Bool, String, one-scalar String respectively |
| signed/unsigned integers | Exact normalized Integer; range-checked |
| f32/f64 | Float; f32 widens exactly, narrowing is checked |
| byte serialization | Bytes |
| sequence/tuple | Array |
| string-keyed map/struct | Map |
| unit/unit struct | Null |
| newtype struct | Its inner representation |
| option | None is Null; Some is its non-null inner representation |
| ordinary externally tagged enum | Unit variant is a string; payload variant is a one-entry map from variant name to payload |

`Some` whose encoded payload is Null fails in this structural profile. This
includes `Some(())` and `Some(None)`. It must not silently become indistinguishable
from None. An explicit tagged option wrapper is provided for such values, using
an application-visible representation such as `{"kind":"none"}` and
`{"kind":"some","value":null}`. Nested wrappers remain distinguishable.
Changing an existing schema to use the wrapper is a schema migration.

Serde attributes such as internally/adjacently tagged enums, flatten, and custom
serialization determine the shape actually presented to the bridge. Publish a
tested support matrix; reject duplicate keys and unsupported shapes. Do not
promise recovery of original tuple widths, struct names, or newtype identities
from an untyped Value alone. A `Vec<u8>` is ordinarily a sequence: callers must
use a byte-oriented Serde type/adapter when they mean Bytes.

No first-class enum/variant node is added in this release. Versioned tagged
structures suffice for Horns views and operation states, and remain inspectable
with ordinary paths. A future richer type-preserving profile must use an explicit
entry point and encoding identifier. It may not quietly alter `to_value`.

### 6.3 Encoding profiles

| Profile | Fidelity contract |
|---|---|
| Plain JSON | Structural subset; rejects Bytes and non-finite floats; integers decoded exactly within the IR range |
| StructFS Value JSON v1 | Explicit typed envelope; preserves all IR distinctions modulo declared normalization |
| CBOR value profile | Native supported IR representations, normalized integers/NaNs; rejects unsupported tags and non-string map keys |
| FlexBuffers value profile | Native representations except maps with embedded NUL in keys; explicit size/depth limits and unsupported-value errors |
| Raw Record | Preserves exact bytes and format identity without semantic conversion |

Plain JSON must not silently convert large integers through f64. Consumers using
JavaScript numbers need a declared safe-integer restriction or the lossless
profile; JSON syntax alone does not guarantee their precision. Integer and float
spellings must decode according to the declared profile. Plain JSON float output
must preserve its Float classification when read by the StructFS JSON codec.

The lossless JSON format has a dedicated proposed media identifier,
`application/vnd.structfs.value+json;version=1` (not yet a registration claim).
Its grammar is a fully tagged
tree, not special keys opportunistically recognized in ordinary user maps:

```text
document = ["structfs-value", 1, node]
node = ["null"]
     | ["bool", boolean]
     | ["int", decimal-string]
     | ["float", binary64-bits-as-16-lowercase-hex-digits]
     | ["string", string]
     | ["bytes", standard-padded-base64-string]
     | ["array", [node, ...]]
     | ["map", [[string, node], ...]]
```

Decimal strings have no plus sign, leading zeros, or negative zero and must fit
the IR range. Map entries are unique and emitted in UTF-8 byte order. NaNs use
bits `7ff8000000000000`; decoding normalizes any accepted NaN representation.
Float zero sign survives. Reject unknown tags/versions and malformed arities.
Because every user array/map is wrapped, a user value resembling this envelope
cannot be confused with the envelope itself. Profile selection is explicit;
ordinary JSON objects are never auto-detected as tagged IR values.

The detailed value specification fixes JSON escaping, whitespace, and canonical
validation so this envelope has a unique canonical encoding. Hash algorithms and
domain separation still require a separate protocol. Codec profiles, schema
versions, and the Wasm ABI version are separate identities.

### 6.4 RON lessons and authoring support

RON demonstrates that direct typed serialization can support shapes a generic
Value intermediary cannot reconstruct. Its documentation does not guarantee
round trips through `ron::Value`, and that Value's deserializer does not support
enums. Serde itself distinguishes more shapes than a structural tree.

References checked on 2026-09-11:

- [RON limitations](https://github.com/ron-rs/ron#limitations)
- [RON Value](https://docs.rs/ron/0.12.2/ron/value/enum.Value.html)
- [RON Number](https://docs.rs/ron/0.12.2/ron/value/enum.Number.html)
- [Serde data model](https://serde.rs/data-model.html)

RON is a possible later authoring frontend for assemblies/configuration. Such a
frontend needs a typed schema or explicit lowering rules. Comments and source
locations belong to an authoring representation, not runtime Value. Arbitrary
RON-to-Value fidelity, comment-preserving editing, and a new textual language
are deferred. The release must prove the entire IR-mediated round trip, rather
than only typed encode/decode directly through a format.

## 7. Paths, errors, enumeration, and discovery

Retain validated component-wise paths. The core Wasm v1 binding continues using
joined UTF-8 paths, which are unambiguous under its grammar. Direct Path Serde
uses that canonical string; supply an explicit components adapter for existing
Horns/transport schemas. Version existing records when changing their encoding.
Do not impose a single representation on previously versioned transports.

Keep `PrefixSuffix` meaning zero-or-more middle components. Add an explicitly
named constrained pattern or minimum-middle count for consumers requiring one
or more. Serialize patterns with explicit kind and constraint; migration must
not broaden subscription or capability matches accidentally.

Typed error categories survive native clients, provider middleware, guest SDKs,
and transports. Messages are bounded diagnostics and must not drive control
flow. Extend service error details for expired cursors, unsupported profiles,
revision conflicts, and commit-unknown outcomes where needed. Map these through
existing Isotope categories using structured service results when the v1 binding
cannot carry richer details. Do not break the two-import ABI to add diagnostics.

Enumeration is an optional service operation expressed through read/write
protocols, with pages and opaque cursors. It must not require reading an entire
subtree, consuming a stream, or triggering an application action. The existing
`read_children` default is only a helper for documented structural stores;
deprecate universal use and ensure native/guest clients use the same profile.

Service profile discovery is explicit host/assembly metadata, available without
arbitrary application reads. Profile descriptors identify version, supported
operations, limits, and blocking/consuming behavior. Do not inject a globally
reserved `meta` child into every user data tree. `iso/capabilities` continues to
describe grants, not complete provider schemas or secrets.

Authority applies to aggregate reads as well as leaf paths. Snapshot pages,
enumeration, change payloads, and embedded references must not reveal data outside
the grant or projection. Filtering a leaf read is not sufficient when an ancestor
read can return that leaf inside a map. Service implementations validate paths
inside request envelopes; routing access to `operations/` alone does not grant
authority over every path that an envelope can name.

## 8. Async routing and host composition

The supported composition layer distinguishes:

1. Sync stores for short local work and immutable snapshots.
2. Detached providers that construct a future promptly, release their dispatch
   lock, then wait independently.
3. Cloneable client handles that issue concurrent calls through a scoped router.

The shared router owns capability checks, component-wise prefix mapping,
returned-path rewriting, call context propagation, admission, and tracing.
Featherweight block mailboxes are one target kind; ordinary native providers are
another. Keep one implementation of these rules. A scoped return path must stay
inside the granted target subtree or be rejected; arbitrary redirects cannot
escape authority.

Provide clearly named immediate-sync and blocking-executor adapters. Neither may
be advertised as making arbitrary synchronous code cancellable. Already running
blocking work retains its owner and budget until it finishes. Bare sync calls
from async worker threads must not be the default I/O integration path.

Supply a native async service API with service terminology; using it must not
require implementing `WasmBlockDriver`. Existing artifact adapters remain
supported. Main-thread-affine UI objects stay behind host message adapters; the
router must not require moving a terminal/DOM object to arbitrary worker threads.
The protocol remains usable by a browser implementation without Rust Send/Sync
requirements leaking into wire schemas.

Every accepted call is charged once per applicable ownership/budget level,
including calls to direct providers. Existing mailbox charging must be reconciled
with router admission to avoid accidental duplicate charges. Provider-specific
charges cover effects and retained buffers beyond the routed call's lifetime.

## 9. Revisioned state and change service v1

### 9.1 Scope

Ship one in-memory reference provider and its wire/guest/native clients. It is a
tree state service with local atomicity, not a distributed transaction manager.
No operation claims a globally consistent snapshot across unrelated mounts.
Applications may construct a projection provider to establish a consistency
boundary around data they own.

The service has separate data and control paths. Under its grant root, `data/`
contains user state and `operations/` accepts versioned command envelopes.
Returned handles live at `outstanding/{id}`. User keys therefore cannot collide
with operation names. Raw arbitrary records are not accepted as state nodes;
they must be decoded to Value under an explicit codec policy or stored as Bytes
with application metadata.

Proposed requests to `operations/`:

```text
{version: 1, op: "snapshot", prefix, limits}
{version: 1, op: "observe", prefix, limits}
{version: 1, op: "watch", prefix, after: {epoch, revision}, limits}
{version: 1, op: "batch", expected: {epoch, revision}, mutations: [...]}

mutation = {op: "set", path, value}
         | {op: "delete", path}
```

Relative paths in requests are relative to the authorized state data root, not
the router's global namespace. `expected` may be omitted for unconditional local
batches. A set replaces a subtree; a delete removes it; setting Null stores Null.
Mutations apply in request order atomically. Empty batches are rejected. Batch
size, node depth, bytes, and handle counts are bounded before acceptance.
For v1, `data/` is read-only and all mutations use the batch operation, so no
direct-write bypass can omit revision/change publication. Conditional updates
use the provider revision in v1. Application generation checks occur against a
snapshot and commit with that snapshot's expected revision; a concurrent change
therefore forces revalidation. A dedicated field predicate can be added later.

### 9.2 Revisions and atomic publication

A revision token is `{epoch, revision}`. Epoch identifies a provider incarnation;
revision is a monotonic u64 commit number. Tokens from another epoch cannot be
silently compared or reused. Overflow fails admission before mutation.

The provider validates authority, budget, expected revision, and all mutations,
then commits state and its change record at one serialization point. Failure
before commit changes neither state nor revision. Every accepted nonempty batch
increments revision, including a batch whose final values compare equal. Change
records may indicate no net changes. Command occurrences must never be inferred
solely from value inequality.

The result is `{committed: true, token}`. A successful response means the batch
committed within this provider's declared durability level. Cancelling after the
commit does not undo it. A lost response can leave the caller uncertain; unsafe
automatic retries are prohibited. Idempotency receipts are an optional declared
provider extension, mandatory for any application promising retry-safe effects.

### 9.3 Snapshots and observation

A snapshot pins an immutable view at a token. Paged reads from its handle all
observe that revision. A terminal page includes its cursor and terminal status.
Snapshot memory, age, page bytes, and live handles are bounded. Exceeding a bound
returns a typed error; the provider never silently changes the pinned revision.

`observe` atomically creates both a snapshot and a change cursor starting after
that snapshot's token. It returns handles for both. The provider reserves the
bounded observation resources before creating them. This closes the gap between
fetching initial state and subscribing. If the consumer falls behind while
reading the snapshot, retention expiry is reported explicitly and it must restart
observation; unbounded retention is not promised.

`watch` resumes after a known token only if that history remains available.
Reads return bounded pages with `{items, next, done}`. Items represent committed
changes and carry their tokens. `next` can advance over filtered-out commits;
the provider must not repeatedly scan the same irrelevant history. A future
revision is invalid, a different epoch requires resynchronization, and an expired
revision returns `CursorExpired` with the earliest available token. No clamping
of invalid/expired cursors to the current tail.

The provider advertises a maximum atomic change-record size and minimum usable
page capacity. It rejects batches whose retained change record cannot fit its
configured bound before commit, and rejects incompatible observer page limits
at creation. Pages never split an atomic change record. Slow observers may be
expired to reclaim history; they must not pin unbounded memory or indefinitely
block writers. Oversized snapshots similarly fail admission rather than returning
an undocumented partial view.

Provider-owned change records are authoritative. An adapter observing arbitrary
read/write calls can emit operation traces or invalidations but cannot label
best-effort before/after reads as committed state. Consuming reads, computed
values, and writes bypassing a wrapper make that claim unsound.

### 9.4 Reactive application semantics

The reference reactive application processes accepted input in order against a
known state revision. A reducer/command returns a state batch and descriptions of
effects to start after that batch commits. A failed conditional commit does not
launch effects. The application decides whether retrying a pure reducer is safe.

Effects receive an owner, generation, restricted client, deadline, and budget.
Supersession requests cancellation and invalidates the prior generation. Results
are conditionally committed against the current generation; cancellation alone
cannot prevent an already dispatched stale write. This requires a provider-local
conditional transaction, not a client-side check followed by an unrelated write.

State commit followed by task start is not crash-atomic. Ephemeral UI effects can
be retriggered from state after restart. Durable workflows require an application
outbox/intent or equivalent reconciliation protocol. The reference in-memory
service does not claim durable exactly-once effect execution.

Input/commands use a reliable ordered queue with explicit admission failures.
Render invalidations may coalesce and presentation may use the latest committed
projection. Durable events retain their application's history/cursor contract.
Bound work by total operations, fan-out, bytes, and time; a cascade-depth bound
alone does not bound a branching cascade. Observer failure never retroactively
turns a committed write into an uncommitted one.

Renderers read immutable projections through a synchronous Reader. They do not
perform live network reads or consume event streams. Projection schemas should
expose necessary facts such as credential presence without credential material.

## 10. Ownership, cancellation, and budgets

### 10.1 Owners

Use explicit owners for host/service, instance, request, operation, subscription,
and presentation session. Ownership is not implied by a descriptive name or by
the lifetime of an arbitrary borrowed reference. Registrations receive unique
opaque identities; labels and mount prefixes are diagnostic/schema information.

An owner tracks its child tasks, provider operations, reservations, registrations,
and handles. Drop initiates nonblocking cancellation; explicit async close joins
work and returns remaining resources. Applications retain a cleanup supervisor
through shutdown. Noncooperative native work is reported and stays charged.

Requests may address persistent instances. Request cancellation closes that
request's admissions, removes its pending correlations, and signals its owned
work without stopping unrelated requests. Work that must survive a request is
created under an explicitly named service owner at creation time; there is no
silent detached-task escape.

### 10.2 Downstream effects and cleanup

Opening a provider handle records ownership before it is exposed to the caller.
If result delivery is abandoned, the host still owns cleanup of the created
handle. If an operation commits an irreversible effect and its result is lost,
the status is uncertain until queried/reconciled; cancellation must not claim
the effect never happened.

Cleanup uses host-owned cancellation/release authority outside the expired
request's ordinary call scope. It remains capability-limited and has a reserved,
bounded cleanup path. Guest cleanup writes are helpful but not the only release
mechanism: a trap or immediate interruption can prevent them entirely.

Provider contracts identify the cancellation point and distinguish:

- Not dispatched: admission/cancellation can prevent the operation.
- Dispatched but cancellable: signal cancellation and join/observe completion.
- Committed or noninterruptible: preserve outcome/remaining work and reconcile.

This extends the existing Featherweight shutdown report; it does not weaken the
requirement to retain reservations until work is joined.

### 10.3 Accounting

Bound guest slots/memory reservations, routed calls, direct provider calls,
request queues, observer history, snapshots, stream buffers, retained results,
timers/signals, logs, and background tasks. Each resource has one documented
owner and release event. Logical payload bytes, reserved memory, measured Wasm
memory, fuel, and process RSS are different quantities.

Live admission policy updates preserve charges and affect new admissions.
Execution fuel/memory policy changes are separate. Caps do not promise scheduling
fairness. A stalled screen, provider, or tenant must not consume all capacity
reserved for peers or for cleanup.

## 11. Capability service profiles

Extend Isotope spec 13 with independently versioned profiles. External capabilities
remain explicit grants outside `iso/`. Profiles describe schemas, fidelity,
blocking/consuming behavior, errors, ownership, and limits. Supported profiles
must be discoverable without probing effectful application paths.

| Profile | Next-release commitment | Later work |
|---|---|---|
| State/change | Section 9 protocol, in-memory reference provider and clients | Durable backend adapters, cross-provider projections |
| Operation handle | Start, bounded status/result, cancel, release, terminal state; ownership contract | Provider-specific idempotency and recovery integrations |
| Binary stream | Reuse bounded duplex implementation; EOF/readiness/half-close/release conformance | Specialized zero-copy transport optimizations |
| Interactive session | Versioned ordered input envelope, resize/paste/key/mouse/close, exclusive presentation ownership; headless reference adapter | Production terminal/DOM adapters, IME/accessibility extensions |
| Configuration | State profile plus explicit commit/persistence acknowledgment and validation outcomes | General file-editing frontend and source-preserving tooling |
| Process execution | Publish profile requirements and fixture-backed fake provider: environment/workspace grants, streams, exit, cancel/join | Production OS adapters and platform-specific sandbox certification |
| Network/filesystem | Preserve explicit grant boundaries and existing providers; document capability/ownership requirements | Universal listener manager, richer file service, filesystem durability profiles |

Interactive input has a version, session identity, sequence, and typed payload.
Successful input admission is not proof the UI has rendered it. Expose processed
sequence and rendered state revision separately. Presentation is exclusive per
host surface; closing one session cannot unregister another. View payloads are
application schemas (Horns owns its View), not core IR variants.

Configuration save returns whether the provider has acknowledged persistence,
not merely whether a callback was scheduled. The durability level must be named.
General state observation has no implicit fsync guarantee.

Process cancellation must describe descendants/process-group handling and
joining. Native adapters are trusted host code; their use does not create an OS
sandbox. A fake-provider fixture verifies protocol behavior, not platform security.

## 12. Isotope and Featherweight changes

Preserve the core-Wasm v1 two-import ABI. Update its specification and SDKs to
name supported value/encoding profiles and retain typed error categories.
Application messages continue through reads/writes and the existing mailbox.
Serialization negotiation must reject unsupported profiles before guest effects.

Bring docs/spec 06 (values/errors), 03 (namespaces/enumeration), 07 (server calls),
11 (binding profiles), and 13 (embedding/services) into agreement with this design.
Record any migration affecting fixtures/transcripts separately; crate versions
do not silently change transcript or transport formats.

Featherweight reuses the shared router and ownership infrastructure. Keep prepared
artifacts, fresh executions, persistent instances, requests, and shutdown reports.
Add native async service ergonomics and per-request downstream context propagation.
Guests serving persistent requests need SDK helpers that associate provider work
with the response identity; polling a cancellation bit alone is insufficient to
interrupt an already parked provider call. Implement this through an explicit
request-scoped service/client protocol or adapter context, with conformance tests,
without implicitly assigning unrelated instance work to the latest request.

Do not convert every screen widget or store into a guest. Block boundaries should
follow isolation, ownership, deployment, and resource-policy needs. A screen
controller can own a local projection and serve multiple ordered inputs; a
gateway or agent turn can use fresh execution against longer-lived providers.

The browser host implements the same declared protocol profiles through its own
execution mechanism. Publish a native/browser support matrix. Worker/SAB residency
requirements and missing host capabilities remain explicit; same guest bytes do
not imply identical host features or scheduling guarantees.

## 13. Priority and dependency order

Implementation reports: [P0-A Value v1](../specs/value-v1-implementation.md) and
[P0-B shared async services](2026-09-11-shared-async-services.md).

| Priority | Deliverable | Completion criterion |
|---|---|---|
| P0-A | Value IR, direct Serde bridge, typed errors, encoding profiles | Fidelity matrix and adversarial conversion corpus pass |
| P0-B | Shared async router/client/provider contracts | Native and guest callers exercise identical routing, authority, cancellation, and admission behavior |
| P0-C | [Implemented: owned registrations, provider work, cleanup, bounded tails/results](2026-09-11-owned-services.md) | Independent lifecycle and overload fixtures pass; no unaccounted detached work |
| P0-D | Revisioned state/observation reference implementation | Atomic batches, snapshot/watch handshake, expired cursors, stale-result rejection pass |
| P0-E | Isotope profile/SDK alignment and application fixtures | Three archive-built external consumers pass documented scenarios |
| P1 | Production interactive/process adapters; RON authoring exploration | Domain-specific integration and platform tests, without changing core contracts |
| P2 | Richer typed IR profile, stable canonical hashing, advanced inspection | Separate specification and independently versioned acceptance vectors |

P0-A establishes data contracts used by all later work. Router and ownership work
can be developed together; observation depends on both. Start the consumer
fixtures early as executable requirements, then complete them against archives.
Do not publish a nominally complete application target with P0 behavior described
only in documentation. If scope is reduced, rename the release commitment and
state exactly which infrastructure Ox must continue to own.

Deferred: full POSIX/WASI compatibility, universal snapshots of live interpreters,
automatic distributed restart/placement, cross-mount transactions, arbitrary
CBOR/RON feature preservation, and a universal UI framework.

## 14. Acceptance and conformance

### 14.1 Value/codec matrix

For every supported profile, verify `decode(encode(v))` is semantically equal to
normalized `v`; unsupported values fail explicitly. Separately verify
`Rust T -> Value -> encoding -> Value -> Rust T` for the declared typed subset.

Include null/absence, nested maps/arrays, bytes versus numeric arrays/strings,
signed and unsigned boundaries, float/integer distinction, negative zero,
infinities/NaNs, UTF-8 keys, duplicate keys, depth/allocation limits, invalid tags,
unknown profile versions, all enum payload forms, Serde tagging/flattening,
byte adapters, and the rejected/explicitly wrapped nested-option cases.

Test identical values through direct typed conversion and each codec. Do not use
JSON conversion as the oracle for binary formats. Verify raw Record forwarding
byte-for-byte separately. Property tests/fuzzing must test intermediate Values,
not only direct typed format round trips.

### 14.2 Reactive-screen external consumer

Build a small Horns-shaped application without importing Ox. Use a serializable
view schema, state provider, ordered input, immutable render projection, and a
fake asynchronous catalog operation. The same reducer/projection behavior must
be exercised natively and through a core guest.

Required scenarios:

- Two installations with equal labels at different prefixes; close one and keep
  the other operational.
- Repeated identical keys are processed separately and in accepted order.
- Immediate effect completion cannot precede the committed pending state.
- Superseded effect results fail conditional commit, including a cancellation race.
- Snapshot plus observation has no undetected gap; history overflow causes resync.
- Render coalescing does not lose input acknowledgment or state commits.
- A stalled renderer/provider does not block another screen.
- Projection grants reveal credential presence without bytes or unauthorized paths.
- Closing the session joins its tasks and releases subscriptions, snapshots,
  cursors, presentation ownership, and budget charges.

### 14.3 Streaming-gateway external consumer

Use a shared prepared artifact, fresh executions, bounded fake upstream, and a
response consumer. Exercise native/guest format boundaries and real router imports.

Required scenarios: slow consumer, terminal-tail race, admission exhaustion,
oversized payload/response, client disconnect before and after downstream handle
creation, timeout, fuel exhaustion, guest trap, failure delivering an open result,
and noncooperative native provider reporting. Downstream cleanup must complete
without relying on a guest's final writes. Direct-provider accounting is asserted.
Measure actual workload latency/memory separately from synthetic session capacity.

### 14.4 Conversation-service external consumer

Use a persistent service owner, fresh turn executions, a fake tool operation,
approval wait, and a small durable test provider. Prove commit acknowledgment
precedes published events; snapshot/config persistence has a separate boundary.

Required scenarios: observer disconnect does not cancel the turn; turn cancellation
does cancel its tool; service shutdown joins both; approval requires matching
operation identity; restart/reconnect rejects old epochs or resumes supported
durable cursors; duplicate application operation IDs obey the fixture's explicit
idempotency contract. A successful read/write alone is not proof of exactly-once
remote execution.

Ox's actual ledger, approval, remount, worker and transport suites remain migration
acceptance tests in Ox. The generic fixture does not replace their domain checks.

### 14.5 Packaging and host matrix

Extend `scripts/check-featherweight-release.py` to build all three consumers from
extracted Cargo archives without sibling-repository or source-checkout dependencies.
Verify minimal feature combinations, core guest SDK modes, docs, and the selected
MSRV/targets. Include a current-thread executor scenario to expose hidden blocking
bridges and a browser test of the interactive protocol profile. Platform-specific
adapter support must be stated, not inferred from a native test.

Release evidence records commands, results, crate/ABI/profile versions, host
conditions, and excluded tests. Prior successful runtime tests are baseline
evidence only. This specification itself changes no executable code and does not
constitute a completed release gate.

## 15. Ox migration target and compatibility

Ox may adopt the supported core-Wasm SDK and profile schemas directly. Keeping its
three-import guest ABI is optional adapter work, not a release prerequisite.

1. Move common values, typed conversion, patterns and errors onto released crates.
2. Move native store routing and background ownership onto the supported service
   layer, preserving explicit durability and policy boundaries.
3. Move Horns subscriptions to state observation and ordered command/effect
   processing. Keep View, bindings, renderers, and command semantics in Horns.
4. Move gateway runners to prepared Featherweight executions with host-owned
   downstream cleanup; retain or migrate provider schemas explicitly.
5. Replace the agent execution boundary while retaining conversation namespaces,
   ledger commit rules, approval semantics, turn bookkeeping, and remote ingress.

This is migration sequencing guidance; implementation plans require current-source
verification in the target repository.

Coordinate a breaking crate version where signatures or semantics change. Publish
a migration table for JSON conversion failures, unsigned values, Null/delete
profiles, pattern matching, enumeration, error types, registrations, and request
ownership. Do not silently reinterpret existing configuration, transport,
transcript, or ledger data. Supply explicit adapters/readers where retaining old
data is required. New profile identifiers and fixture vectors must ship in the
archives.

## 16. Review decisions that do not change the contract

Before implementation, select final service crate names, numeric Rust storage
layout, concrete owner/client API names, and the first production interactive
adapter. These choices must satisfy the behavior above and do not justify
weakening the release fixtures.

If consumers require transparent round trips for arbitrary Serde options,
non-string maps, or format-specific tags, revise section 6 explicitly before
implementation. Do not silently grow a reserved-map convention or claim the
structural profile is a universal Serde AST.
