# Application capability profiles v1

These profiles version independently of the two-import core-Wasm ABI. Portable
Rust schemas live in `structfs-profiles`; the state schema lives in `structfs-state`.
All payloads use StructFS Value v1. Profile versions are positive integers, not
crate versions. This release implements version 1.

## Scope: optional store contracts

StructFS core defines paths, values/records, read/write results and errors, and
composition. The stores implementing functionality provide all higher-level
semantics: consistency, transactions, durability, observation, streams, operations,
configuration workflows and process policy. Implementing StructFS does not require
implementing any profile in this document.

These profiles are optional, reusable contracts for stores that choose to implement
them. Requirements below apply within the declared profile, not to arbitrary stores
or to the StructFS core. A store can expose a different documented contract without
adopting these schemas or paths. Isotope and Featherweight provide routing,
execution and lifecycle machinery; they do not strengthen a store's guarantees.

## Discovery

A granted provider may expose pure, read-only `meta/profiles`:

```json
[{"profile":"structfs.interactive","version":1,"implementation":"reference"}]
```

`Profiled` reserves that relative path, rejects writes, and forwards other calls.
Its declarations are bounded to 16 unique profiles; duplicate profiles or versions
other than 1 are rejected. `reference` identifies a reusable implementation;
`compatibility` identifies an explicitly documented older convention; `fixture_only`
identifies executable contract examples without a production adapter promise.
Declarations are provider assertions, not runtime certification. Unknown required
versions must fail explicitly; absence is not evidence that a profile is supported.
The guest `profiles` feature exposes `sdk::profiles(root, codec)` and portable types.

## State: `structfs.state`

The complete contract is [revisioned state](../../docs/design/2026-09-12-revisioned-state.md).
One owner retains bounded state, revision history and opaque read handles. Commands
open batch receipts, snapshots, watches or atomic observe handles. Tokens contain
an opaque epoch and u64 revision. Old epochs and expired cursors are typed faults;
resynchronization obtains a new snapshot. Batch mutations publish atomically under
an optional expected token. Empty/repeated events are occurrences, not value-diff
notifications. Projection is synchronous, bounded, immutable application data.
The in-memory reference store supplies no disk persistence. The state profile alone
makes no durability promise; another store implementing it may guarantee durable
commits through its own documented acknowledgment and recovery contract.

## Operation: `structfs.operation`

An application-specific start write validates its request, reserves admission and
returns a handle path. A provider must install ownership before returning it and
must not automatically retry start after failed result delivery. The reference
`OperationHandle::start` supplies the host-side handle; the granting application
chooses and registers its confined path. It does not implement a universal job queue.

| Handle operation | Result |
| --- | --- |
| read `status` | operation identity, phase, cancel_requested, joined, result_bytes |
| read `result` | absent while pending/running; bounded Bytes when completed; error on failure or release |
| write `cancel`, Null | idempotent cancellation request |
| write `release`, Null | idempotent cancellation and retained-result release |

Phases are pending, running, completed and failed. Completed/failed are terminal;
a cancellation request need not become a failure if work commits or completes.
Result reads are repeatable while retained. Oversized results fail; diagnostics
are bounded generic errors, not unbounded callback text. Applications encode their
result schema inside the Bytes and declare its encoding. `joined` reports that the
supervised task has actually ended, independently of its outcome. Panic cleanup
failures remain visible in the owner report until explicitly reconciled.

The host reserves result capacity before invoking work. Release clears the retained
result, but cancellation-ignoring work keeps its reservation until it ends. Owner
close reports such work; it never falsely reports it reclaimed. The callback is
trusted host code and must independently bound its working memory. No portable
profile can forcibly stop arbitrary native code or promise rollback.

## Binary stream: `structfs.binary_stream`

Reuse `structfs-handles::DuplexStream`: read `rx/{max}` consumes Bytes; empty Bytes
means EOF; write `tx` transfers Bytes with bounded backpressure. `ready/read`,
`ready/write`, `ready/both` park for advisory readiness. Null to `shutdown` closes
transmit after accepted data, and Null to the root releases both directions. EOF
must not strand the final readable chunk. Oversized writes fail before transfer;
pending-write cancellation transfers no bytes. Provider ownership cleans up both
ends independently of guest final writes. Readiness does not reserve data or space.

## Interactive: `structfs.interactive`

A host grants one exclusive presentation surface per session. Display labels are
not identities or mount prefixes. `HeadlessHost` allocates opaque session IDs and
reserves queue bytes under the session owner. Input envelopes have exactly
`version`, `session`, `sequence`, `input`. Sequence is u64, starts at 1, and must be
exactly the next accepted sequence. Unknown fields and variants are rejected.

| Input type | Fields beyond `type` |
| --- | --- |
| key, paste | text: Unicode String |
| resize | columns, rows: nonzero u32 |
| mouse | x, y: u32; button: u8 |
| close | none |

The v1 key shape carries application key text; it does not standardize terminal
escape decoding, platform keycodes or modifier encoding. Such adapters must declare
their own convention or a future profile version.

Write `input` submits an envelope. Admission checks both event count and logical
byte capacity (64 + session UTF-8 bytes + text UTF-8 bytes per event). It includes
the single in-flight input. Rejection does not advance the sequence. Repeated equal
inputs are separate events. Accepted close prevents subsequent input admission.

Read `input/next` consumes one event and waits when empty. Another read before its
processing acknowledgment fails. Write `processed` with its u64 sequence only after
the reducer commits state and effect intent. Write `presented` with a state Token
after rendering; revisions may coalesce, but cannot move backwards or change epoch
within that session. A new epoch needs a new presentation session. Processing and
rendering are independent counters. `status` reports session, accepted, processed,
rendered and closed. Write `release` requests cleanup. Cleanup revokes input, wakes
waiters, clears retained events and releases the exclusive surface.

Renderers and reducers are application-owned. The TypeScript headless model uses
bigint for sequences/revisions and the same Rust input corpus. Browser embedding
must enforce surface ownership and connect its queue to the host transport; this
model alone does not certify browser worker residency or DOM rendering.

## Configuration: `structfs.configuration`

A configuration store defines what successful reads and writes mean, including
validation, consistency, persistence and recovery. A durable store may make ordinary
write success its durable acknowledgment. StructFS does not require a separate save
operation, a draft state layer, or a persistence acknowledgment Value. Separating
editing from saving is an optional workflow implemented by the chosen stores and
application.

The `structfs.configuration` v1 schema is one optional store-level acknowledgment
convention, used by the fixture. `CommitAck` contains `token`, `persisted` and
`durability`: `memory` pairs with persisted=false; `file_synced` pairs with
persisted=true after the store's file sync succeeds. Consumers of this convention
must reject inconsistent combinations. These are the levels represented by this
particular schema, not a universal durability taxonomy. Other stores can define
transactional, replicated or other guarantees through their own contracts.

The store produces and enforces its acknowledgment. The runtime transports the
result or error without inventing a stronger durability guarantee, retrying effects
automatically, or treating cancellation as rollback. The schema validator checks
field consistency; it neither performs persistence nor verifies that it occurred.

In the conversation fixture, an in-memory configuration snapshot and the journal
are separate stores. The journal validates duplicate operation IDs against identical
content, appends and syncs before acknowledging; the application then publishes
observer events. Lost event delivery does not undo that store commit. Recovery
assumes complete records. Torn-write recovery, atomic replacement, directory sync
and concurrent transactions belong to a production store's implementation and
contract; they do not require changes to the core or runtime.

## Process: `structfs.process`

Process support is fixture-only in this release. The portable request names version,
operation identity, program, args, environment_grant and workspace_grant. Grant names
are opaque host capabilities, never permission to inherit ambient environment or
resolve arbitrary host directories. A production provider must define executable,
environment, workspace, stream, exit and cancellation policy explicitly.

The conversation fixture implements a bounded fake echo process with duplex I/O,
a bounded operation exit result, explicit grant checks, cancellation and owner join.
Its approval-gated fake tool separately demonstrates durable commit ordering and
fresh guest turns over a persistent host service. It does not spawn an OS process,
provide an OS sandbox, or certify Ox worker isolation. Production adapters remain P1.

## Store conventions and implementation state

The revisioned-state batch protocol distinguishes Set and Delete so internal
state and immutable snapshots can represent present Null. An Isotope-facing
conventional store assignment maps a Null write to Delete and another Value to
Set. Its implementation can retain the richer batch operations. This boundary
translation does not add a core operation or require recursive normalization of
Values, command payloads, or snapshots.
