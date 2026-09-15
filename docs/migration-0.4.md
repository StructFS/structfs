# Migrating to 0.4

0.4 is the coordinated StructFS/Featherweight candidate implementing the Isotope
2026-09-14 specification candidate. See [registry status](release-status.md) for
verified publication information; publication and validation are separate facts.
Value JSON v1 and profile encodings remain independently versioned.

## Recoverable hosting

Prepare code once with CoreWasmEngine, wrap the prepared block in Arc, and call
`start_sync` or `start_async` with a CleanupSupervisor, host, codec, format and
ExecutionPolicy. Keep the returned ExecutionOwner until `join` returns an
ExecutionOutcome. The execution result and recovered host are separate fields,
so a trap or cancellation does not discard accepted effects.

`wait(timeout)` returns None while work remains and leaves the owner joinable.
Dropping a wait does not cancel; `cancel` does. Dropping the owner requests
cancellation and leaves unfinished work under the supplied supervisor. A blocking
host or noncooperative async host can delay recovery indefinitely. A host panic
sets `host_panicked`; inspect the returned state but do not assume its invariants
are safe for another turn. Keep the supervisor and executor alive through cleanup.

Configure epoch cadence on the engine. Per-run memory limits may tighten its
ceiling. Growth denial traps by default; choose GrowthFailure::ReturnFailure to
allow Wasm's failed-growth return. Fuel and deadlines cover instantiation and run;
cleanup has its own bounded wait. CPU cancellation cannot interrupt arbitrary
native code. Owned hosting is currently a native core-Wasm embedding API, not a
browser or component capability.

Artifact adapters implement WasmBlockDriver::execute with the complete
DriverContext. The implicit legacy run/run_async fallback is removed. Public
CoreWasmBlock run entry points are replaced by the owned API. The runtime's
assembly driver uses the same policy/configuration and recovery implementation.

## Shared and typed access

SharedReader and SharedWriter take owned arguments through `&self` and return
Send + 'static futures. They are object-safe, including Arc<dyn SharedWriter>.
DetachedShared adapts a mutable detached provider; the lock protects construction
only. Service Client implements the same interfaces. The provider defines when
an operation is accepted and what dropping its future does; detachment promises
neither rollback nor completion.

DetachedShared invokes the provider during operation construction and inherits
its acceptance/drop behavior. A construction panic poisons the adapter: subsequent
shared and detached calls fail without reentering that provider. Service Client is
lazy: dropping an unpolled future dispatches nothing. Dropping a pending call signals
cancellation and drops the provider future without rolling back effects; retained
provider work must retain its context lease until completion.

| Record returned by provider | Implicit typed read (sync, async, detached) | Explicit codec read |
| --- | --- | --- |
| Parsed Value | Direct typed conversion | Direct typed conversion |
| Raw JSON | NoCodec error | Decode with the selected codec, then convert |
| Other raw format | NoCodec error | Decode if the codec supports it, then convert |
| Absent | None | None |
| Present Null | Apply the target type's Value conversion rules | Same |

Synchronous read_typed no longer decodes JSON implicitly. Use read_as with
JsonCodec where that behavior is intended. Strict numeric and Option conversion
rules have not changed.

## Snapshots and conventional writes

MemoryStore::from_entries accepts validated (Path, Value) entries, preserves Null
and empty containers, and rejects duplicates and explicit ancestor/descendant
overlaps. An empty input is absent; an explicit Null root is present. root() now
returns Option<&Value>. Constructors establish data and do not replay writes.
Ordinary writing of Null still deletes its target. This is the exposed store
convention, not a restriction on internal data structures or all providers.

StateClient::write applies conventional Null-as-deletion using the internal batch
Delete operation. State's explicit batch Set can still store Null. There is no
new delete primitive and no global recursive Null normalization.

## Paths and response envelopes

PrefixSuffix and PrefixSuffixMinMiddle are replaced by one variant:

```json
{"prefix_suffix":{"prefix":"accounts","suffix":"provider","min_middle":1}}
```

Exact and Prefix retain their representations. Matching and the standalone
matches_prefix_suffix predicate borrow components and allocate nothing. Translate
legacy persisted patterns explicitly; the old encodings are not silently decoded.

path! expands hygienically through renamed dependencies and facade reexports.
Expressions remain validated PathComponent values; use try_new and propagate its
PathError, or reexport PathComponent instead of maintaining a duplicate wrapper.
Direct callers of the proc macro should use the core/facade macro instead.

Server read responses must include present:true with a value or present:false
without one. Unmarked responses are rejected. Rebuild older guests and update
handwritten servers; use ok_value and ok_absent rather than handwritten envelopes.
