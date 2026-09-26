# Coherent StructFS and Isotope contracts

Date: 2026-09-14

Status: Implemented and validated as the 0.4.0 candidate on 2026-09-15.
No publication was performed. See [validation evidence](../docs/release-validation-2026-09-15.md)
and [implementation decisions](../docs/design/2026-09-14-coherent-contracts.md).
The sections below preserve the implementation objectives and acceptance criteria.

## Purpose

Make StructFS, Isotope, and Featherweight express one consistent model of values,
operations, concurrent access, and resource ownership. The September 14 Ox
maintainer letter identifies useful acceptance cases, but downstream compatibility
does not constrain the design. Success means fewer semantic exceptions and less
integration machinery, rather than simply adding every requested adapter.

This plan supersedes compatibility-preserving recommendations made in response
to that letter. It follows the completed [consumer contract work](01-consumer-contracts.md)
and refines the earlier [application substrate design](../docs/design/2026-09-11-application-substrate-and-value-ir.md).
The source letter is `local/structfs-featherweight-maintainer-letter.md`; the
requirements below are self-contained because that local file is not a published
dependency.

## 1. Principles and scope

1. Null-as-deletion is the convention Isotope should follow at its exposed store
   interfaces. It is not a restriction on the data structures or mechanisms used
   to implement those interfaces.
2. Keep the read/write model minimal. Do not introduce a generic delete primitive
   when a conventional Null write already expresses the intended operation.
3. Execution termination, operation completion, and resource reclamation are
   separate facts. Cancellation is a request, not rollback or proof of cleanup.
4. Ownership must survive every ordinary failure path. Recovery is part of the
   execution contract, not an application arrangement of channels and Arcs.
5. Shared clients and mutable providers are different useful interfaces. Expose
   both without requiring a native executor or Wasmtime for basic composition.
6. Equivalent helpers have equivalent semantics. Codecs and policies are explicit.
7. Specify observable guarantees in Isotope; keep Rust types, Tokio scheduling,
   and Wasmtime configuration in implementation documentation.
8. Remove superseded interfaces and compatibility branches when they obscure the
   model. Version incompatible contracts explicitly; do not silently reinterpret
   old artifacts or recorded data.

In scope: tree-store semantics and construction, typed and shared APIs, path
patterns, execution ownership and limits, bindings affected by these contracts,
documentation, conformance, and release evidence.

Out of scope: Ox's guest ABI migration, conversation policy, provider protocols,
UI dispatch, subscription cascade policy, disk durability, distributed
transactions, and a universal application framework. Namecode's independent
version and behavior need not change merely because this revision is coordinated.

## 2. Verified starting point

These are source observations from the planning checkout, not fresh execution or
registry verification:

| Area | Existing foundation | Gap |
| --- | --- | --- |
| Values | MemoryStore uses Null as empty root and write-as-delete; revisioned state has optional roots and Set/Delete | Construction and provider-specific semantics need clearer documentation; flat-entry snapshot import is missing |
| Execution | Shared prepared modules, fresh stores, admission, metering, cancellation, provider supervision | Prepared artifacts reject synchronous run; public run results do not recover owned host state |
| Blocking effects | Service adapters retain charges and support joined close | A complete typed host-state recovery contract is missing |
| Matching | Component paths and minimum-middle matching exist | Suffix matching copies paths; public variants duplicate one underlying pattern |
| Sharing | Detached futures and brief-lock composition exist | No minimal standard erased shared writer with owned arguments |
| Typed reads | Direct Value conversion and explicit codecs exist | Implicit synchronous reads decode raw JSON; detached reads reject it |
| Release | Package gates and registry lookup tooling exist | Changelog and Isotope release page still describe 0.3 as unreleased |

Primary implementation locations are `packages/core-store`, `packages/serde-store`,
`packages/service`, `packages/state`, and `featherweight/runtime`. Normative sources
are `isotope/spec/` and `docs/specs/structfs-value-v1.md`.

## 3. Store conventions and snapshot construction

### Separate convention from mechanism

Null-as-deletion is the recommended convention for ordinary tree-store writes,
and Isotope's exposed store semantics should follow it. Writing Null is how a
caller deletes a value there; a separate generic delete operation adds no meaning.
This is an interface convention, not a normalization law applied recursively to
every value, map, database, or provider used to implement Isotope.

Keep Null as a Value and retain the protocol's ability to represent it. An
implementation may use optional roots, explicit Set/Delete mutations, tombstones,
or Null-preserving snapshots internally. Those mechanisms do not dictate the
public interface: an adapter exposing them as an Isotope store implements the
store convention at that boundary. Do not remove an internal Delete operation
merely because the public store spells deletion as a Null write. Likewise, do
not add a core deletion trait, profile, or Wasm import for this purpose. Explicit
command paths retain their documented operation meaning; distinguish those from
assignments to stored values.

Document MemoryStore's conventional write behavior clearly: writing Null deletes
its target subtree, maps replace subtrees, and deep writes create intermediate
maps according to its traversal rules. Conformance tests certify stores that
claim those conventions, including Isotope's exposed stores, not every internal
Reader/Writer implementation. Separately certify internal providers against
their own contracts and test the adapters that expose them.

### Construction need not replay operations

An explicit snapshot constructor establishes initial data. It need not execute
the operations or side effects of writing each entry. Consequently, preserving
Null during construction can be useful even when a later ordinary Null write
would delete that value. Document this distinction instead of normalizing all
providers or stripping Null members from arbitrary input trees.

Provide a fallible flat-entry builder with a clear contract:

- Preserve supplied Null leaves, empty maps, and empty arrays.
- Validate every supplied path through the shared component grammar.
- Reject duplicate paths, including equal duplicate values, and explicit
  ancestor/descendant overlaps regardless of insertion order.
- Explicit maps and arrays are complete values. Do not merge other flat entries
  into them. Implicit parents are maps; numeric path components do not infer arrays.
- Preserve arbitrary keys inside supplied Value maps. A Value map key is not
  automatically a valid addressable PathComponent.
- Build privately and return a complete snapshot or an error identifying the
  conflicting paths; no partial import becomes visible.

MemoryStore currently treats a Null root as absence while preserving Null children
from a prebuilt tree. Resolve and document this constructor-specific edge case:
prefer an explicit optional-root representation if the constructor promises full
snapshot fidelity, while keeping ordinary Null writes as deletion. This is an
implementation choice for MemoryStore, not a new universal presence requirement.
A constructor must not claim fidelity and silently collapse a supplied root value.

Use a trie or ordered-path validation to avoid quadratic all-pairs conflict
checking. No last-write-wins option is needed in the first implementation. Record
array traversal/deletion behavior for each relevant provider; do not impose global
array compaction or hole semantics to satisfy a store guideline.

### Required propagation and evidence

Update MemoryStore documentation and its convention tests, add snapshot import
fixtures, and explain differences from revisioned state where relevant. Keep
blanket normalization out of routing, codecs, and generic shared adapters. Put
any translation required by Isotope's exposed store convention in the responsible
store implementation or semantic adapter, not in unrelated infrastructure.

Test imported Null leaves and root policy, empty containers, conflict order
independence, literal map keys, and subsequent conventional Null writes. Include
an internal provider that preserves Null and an Isotope-facing adapter that
translates a public Null write into that provider's deletion mechanism. Verify
the public result without requiring the internal representation to change. Ox can use snapshot
construction where its semantics fit without requiring all stores to adopt its
persisted representation.

## 4. One execution ownership model

### Public lifecycle

Define four concepts, with final Rust names chosen during implementation:

| Concept | Owns / guarantees |
| --- | --- |
| Prepared artifact | Validated reusable code and engine compatibility; no per-run host state |
| Execution policy | Guest limits, absolute deadline, cancellation, growth-denial behavior |
| Execution owner | Host store, accepted effects, execution task, and outstanding cleanup responsibility |
| Joined outcome | Execution result plus recovered host state and final accounting after owned work ends |

Fresh execution must not imply a persistent guest instance. Preserve persistent
instances as an explicit separate mode with their own lifetime; cancelling one
request must not shut down an unrelated request or shared instance.

The host store is returned on success, nonzero guest exit, trap, fuel exhaustion,
deadline, cancellation, and setup failure after ownership transfer. Recover it
before transfer if admission or policy validation fails. Keep the result field
separate from recovered state: `Result<HostState, Error>` is insufficient.

An execution owner exposes cancellation and a repeatable wait/join operation.
Dropping a wait leaves ownership intact. A bounded wait can report incomplete
cleanup while retaining the same owner. Dropping the owner requests cancellation
and transfers remaining work to an explicitly configured supervisor; it must not
spawn unaccounted detached work or synchronously block in Drop. Admission fails
before starting work if supervision capacity is unavailable.

Return owned host state only once all operations that can mutate it have joined.
Do not require a cloneable store or an application-provided shared-state wrapper.
Effects can be fields of the returned host state; Featherweight need not invent
a universal application effect log. Recoverable state is not a durable checkpoint.

Guarantees cover normal runtime errors and cooperative process lifetime. A panic
inside host code can leave state logically invalid; report that separately from
a guest trap, retain cleanup ownership, and document whether recovered state is
inspectable but unsuitable for reuse. Process abort cannot promise recovery.

### Synchronous and asynchronous hosting

Implement synchronous hosting as a first-class route through this ownership
model. Evaluate whole-run blocking execution against blocking host-operation
dispatch over the async runner using a small actual-Wasm prototype. Choose based
on ownership simplicity, bounded admission, cancellation behavior, and scheduling
cost; do not maintain two independent lifecycle implementations for compatibility.

Synchronous host effects never block async executor workers. Operations on one
mutable host store are serialized, accepted work is accounted for before dispatch,
and close atomically stops new admissions. Specify the acceptance linearization
point and distinguish queued work that can still be cancelled from running work
that must finish. Cancelling a guest does not discard an accepted write's effects.

Bound compilation, execution, queued blocking work, and unfinished cleanup. Retain
each reservation until the resource it represents is actually released; guest
memory and blocking provider work may have different release times. A permanently
blocked host operation remains owned and visible, rather than being declared
reclaimed after a timeout.

Build on service owners and provider supervision. Add extraction/recovery where
needed rather than introducing a second supervisor with a different close model.
Retire raw-run and legacy driver entry points that bypass the final guarantees;
a bytes convenience API should prepare and execute through the same machinery.

### Limits and cancellation

- Configure epoch cadence once per engine, validate it at construction, and use
  an engine-wide ticker that remains live for all executions. Independent runs
  use independent cancellation tokens; ticking does not itself cancel a store.
- Put fuel, absolute deadline, memory cap, and growth policy in a coherent policy
  object. Allow per-run tightening of engine ceilings, not accidental relaxation.
- Make trap-on-denied-growth the default bounded-execution policy. Offer explicit
  Wasm-style failed-growth return where desired. Document which denial causes
  are covered; a memory cap is not a guarantee about host RSS.
- Enforce limits during instantiation/start functions and guest allocation calls,
  not only the exported run function. Bound manifest inspection during prepare.
- Use one absolute deadline through queueing, instantiation, guest execution,
  and host calls. Cleanup has a separately observable wait budget; expiration
  never transfers the store prematurely.
- Adapters must enforce requested guarantees or reject unsupported policy before
  executing guest code. Browser limitations must be explicit and tested.

Acceptance: one compilation and multiple fresh instances; non-Clone state recovery;
write then trap; cancellation before admission and during an outstanding write;
drop of a wait; supervisor transfer; incomplete then successful join; independent
simultaneous cancellation; fuel/deadline failures; ignored denied growth; failure
during instantiate/allocate; saturated admission; and final resource accounting.
Use deterministic barriers for host-operation races rather than timing sleeps.

## 5. Shared access and uniform typed conversion

Make object-safe shared reader and writer traits available in the portable core
async surface. Methods take `&self` and owned Path/Record arguments and return
Send + 'static futures. Support erased Arc handles without requiring routing,
Tokio, or Wasmtime. Keep reader and writer capabilities separable.

Retain mutable synchronous providers because they can own state without interior
mutability. Keep mutable detached construction only if first-party implementations
need that distinct capability. Inventory borrowed async, detached, and shared
traits; remove redundant public families instead of multiplying every helper by
every historical interface. Document the retained roles and conversion costs.

Adapters release construction locks before polling or waiting. Require prompt
construction, document provider-specific acceptance/drop behavior, and avoid
claiming that a 'static future is cancellation-safe. Strong completion guarantees
belong to owned/supervised operations. Do not grant a writer ordering across
concurrent calls that its provider does not implement.

Make implicit typed reads parsed-only across all retained families. Raw JSON,
CBOR, FlexBuffers, and other records require an explicit codec. All conversions
use Value directly and preserve the same typed errors and diagnostic limits.
Absent reads remain None; any provider's explicit Null result follows the
requested type's Value conversion rules. Helpers must preserve the provider's
presence semantics. Do not weaken strict conversion or Option ambiguity rules.

Acceptance: erased shared handles, independent parked calls, construction lock
release, provider errors/drop behavior, and the parsed/raw/absent/Null matrix
across every retained helper family. Preserve portable feature boundaries.

## 6. Paths and pattern normalization

Use Exact, Prefix, and one PrefixSuffix variant with explicit minimum-middle
length. Choose a single canonical tagged representation; retain string Path
encoding unless a separate semantic argument warrants changing it. Component-array
Serde remains a useful explicit adapter, not a second default.

Expose a borrowed prefix/suffix predicate and delegate PathPattern matching to
it. Matching performs component comparisons without path construction, string
formatting, or heap allocation. Avoid overflow and forbid prefix/suffix overlap
from satisfying the minimum length. Certify empty prefix/suffix/path and extreme
minimum lengths with allocation-count tests outside fixture setup.

Complete macro hygiene for renamed direct dependencies and supported facade
reexports. Keep one validated PathComponent type and shared validation grammar.
Use independent compile fixtures for dependency renaming, facade use, invalid
components, and constructor diagnostics. Do not add wrapper-type coercions that
permit unvalidated path construction.

## 7. Specification, bindings, and compatibility removal

Update normative behavior before declaring implementation acceptance:

- Isotope 03/06/07: specify Null-as-deletion at exposed store interfaces, separate
  that convention from internal representations and transport mechanics, and
  remove compatibility decoding/error aliases only where they are redundant.
- Isotope 05/13: execution ownership, request versus instance cancellation,
  recoverable hosting, accepted work, bounded wait, and supervised unfinished cleanup.
- Isotope 14: distinguish public store semantics from profile commands and
  internal mutation/snapshot APIs; specify translations where a profile-backed
  implementation is exposed through the conventional Isotope store interface.
- Isotope 10/11 and determinism 12 where affected: binding projections, declared
  support, operation/result recording, and explicit rejection of incompatible logs.

Update Rust guests, AssemblyScript SDK, component WIT/adapters, browser host,
WASI shim, server helpers, profile clients, and rebuilt Wasm fixtures as applicable.
Keep all supported projections semantically equivalent. Do not change Value v1
encoding merely because a store operation changes; version the contract that
actually changed. A semantic protocol break needs an explicit compatibility
identity or coordinated runtime/SDK pinning and rejection rule, even if import
signatures stay unchanged. Final identifiers belong in the release matrix.

Remove contradictory guidance from README files, CLAUDE.md, site source, examples,
and current migration pages. Preserve dated historical validation as historical
evidence. Publish a concise migration guide rather than indefinite dual behavior.

## 8. Work sequence and review boundaries

| Milestone | Work | Exit condition |
| --- | --- | --- |
| M0: contracts | Inventory implementations; settle snapshot construction, public-store/implementation boundaries, ownership API, and retained trait families; prototype sync execution routes | Decision records and normative drafts specify constructor fidelity, every state transition, and failure ownership; prototype demonstrates recovery after a blocking effect |
| M1: snapshots and conventions | Flat-entry snapshot builder, root policy, documented store conventions and profile distinctions | Snapshot fidelity, opt-in convention tests, and Isotope-facing adapter fixtures pass |
| M2: composition | Shared interfaces, typed uniformity, normalized borrowed patterns, macro hygiene | Independent portable consumers and allocation/compile fixtures pass; obsolete public paths removed |
| M3: execution | Owned execution, synchronous hosting, recovery, supervision, unified limits and engine policy | All execution acceptance cases pass using actual Wasm; incomplete cleanup retains ownership and charges |
| M4: integration | Update remaining SDKs/bindings, consumers, recordings, documentation, and compatibility identities | Native/browser/component projections meet their declared contracts; archives work without workspace patches |
| M5: release evidence | Run final gates and produce version/conformance matrix and migration document | Reviewable release candidate with exact commit, archive identities, results, and declared limitations |

Start the execution prototype in M0; do not leave the highest-risk design until
after the smaller features. Borrowed matching and macro hygiene can proceed after
their contracts settle. M3 can proceed alongside M1/M2 once the ownership/provider
interfaces are fixed. M4 integrates all three; M5 does not accept partial semantics.

These are reviewable implementation boundaries, not a request for parallel agents
or publication. Do not pick a release number by assuming registry state; verify
published versions and assign the appropriate new coordinated crate revision and
independent specification/profile identities during release preparation.

## 9. Verification and release completion

Use focused tests at each milestone, then the existing workspace and extracted
archive gates against the final candidate. Extend those gates rather than creating
a competing release pipeline. Include native and wasm feature graphs, supported
MSRV, Rust/AssemblyScript guest builds, browser execution, and component/WASI
checks at their documented support levels.

Extend the independent conversation-service consumer to prepare once, run
successive turns with a recovered non-Clone backend, preserve a write through a
trap/cancellation, and join cleanup. Extend the reactive-screen consumer for
shared writes, typed behavior, borrowed subscriptions, and explicit snapshot import.
Keep the gateway lifecycle fixture as regression coverage for allocations, late
replies, aliases, and retained resources. These fixtures own small application
policies; they must not depend on Ox's repository or unpublished workspace patches.

Use shared conformance vectors across bindings for absence/Null, mutation,
response envelopes, and typed/profile errors where representable. Benchmark
prepared reuse and synchronous hosting to identify scheduling costs; do not claim
downstream latency improvements from source inspection alone.

Release documentation must derive status from evidence. Before publication,
describe a validated candidate. After publication, verify each package/version in
the registry and reconcile changelog, Isotope release matrix, and site status from
one release record. Represent partial publication explicitly. A local tag or
successful package build is not proof of registry availability. Publication itself
is outside this implementation plan's authorization.

Completion checklist:

- [x] M0 decisions and normative contracts settled.
- [x] Isotope exposes conventional store semantics; snapshot and internal provider contracts remain explicit, with tested boundary translations.
- [x] Shared and typed APIs have one documented meaning per operation.
- [x] Matching allocates nothing and macro expansion is hygienic in supported use.
- [x] Prepared synchronous/asynchronous hosting shares ownership guarantees.
- [x] Every unfinished operation has an inspectable owner and retained accounting.
- [x] Supported SDKs and bindings implement or explicitly reject each guarantee.
- [x] Independent archive consumers pass; fresh validation records identify evidence.
- [x] Superseded APIs and compatibility branches are removed; migration is documented.
- [x] Candidate/release status and independent contract versions are accurate.

The revision is complete when an embedder can compose these contracts without
guessing provider semantics or inventing a local shared writer trait, a second
suffix matcher, or a host-state recovery protocol around Featherweight.

## Completion record

M0–M5 are complete for the supported 0.4 candidate. Whole-run blocking hosting
was selected; mutable sync, borrowed async, detached provider and shared client
interfaces retain distinct roles. File loading prepares once under the supplied
runtime handle. Snapshot construction preserves data; Isotope-facing conventional
writes translate at the store boundary rather than normalizing internal Values.
The validation report records the package hashes, coverage, independent consumers,
platform limits and remaining downstream responsibilities. Publication is separate.

A subsequent [Ox letter audit](../docs/ox-letter-audit-2026-09-15.md) checks each
request against source, records actual downstream subscription tests, and adds
queued-cancellation and panic regressions. Ox application adoption remains distinct
from upstream contract completion.
