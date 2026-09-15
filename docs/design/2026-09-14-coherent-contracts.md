# Coherent contracts: implementation decisions

The 0.4 revision follows the [holistic plan](../../plans/02-coherent-contracts.md).
Compatibility is not a constraint. Isotope's exposed store convention does not
constrain its implementation data structures.

## Stores and construction

MemoryStore keeps an optional root. A new store is absent; importing Null produces
a present Null root. Snapshot construction preserves supplied data without replaying
writes. Ordinary Null writes still delete. Arrays retain Value's existing traversal
and removal behavior; importing an array does not compact its Null elements. No
routing or codec layer strips Null from arbitrary Values.

Flat imports sort by borrowed path components and reject adjacent duplicates or
ancestor/descendant overlaps. Prefix order makes adjacent comparison sufficient;
there is no quadratic all-pairs scan. Every key is already a validated Path.
Explicit container values remain whole; implicit parents are maps.

State's batch protocol keeps Set and Delete. `StateClient::write` is the conventional
assignment adapter: Null becomes an internal Delete; other Values become Set.
The batch protocol can still explicitly store Null. Neither a third primitive
operation nor a new Wasm import is needed.

## Interface roles

Retain mutable sync, borrowed async, and mutable detached provider traits. They
have distinct first-party uses: owned in-memory mutation; sequential namespace
imports borrowing provider state; and prompt construction of concurrent broker
operations. SharedReader/SharedWriter are the client capabilities. DetachedShared
adapts detached providers with a construction-only mutex. Service Client implements
shared capabilities directly. Arc shared clients also implement detached traits,
so existing combinators need no additional shared-specific family.

Only parsed records are accepted by implicit typed helpers. Explicit codecs handle
all raw formats. PathPattern has one struct-shaped prefix/suffix variant; both
constructors and the standalone borrowed predicate use the same matching logic.
The path macro uses a `$crate` wrapper around its proc macro, so renamed dependencies
and facade reexports do not require discovery or a particular dependency name.

## Owned execution

Use whole-run blocking dispatch for synchronous host stores. The actual-Wasm
prototype demonstrated prepared Module reuse, fresh memory, trap/panic recovery,
and accepted blocking effects retained through cancellation. This approach keeps
one host store on one worker and avoids a channel or blocking-pool hop at every
import. The engine's session semaphore bounds dispatched/queued runs. Async
hosting shares admission, policy, store configuration, outcome, and supervision.

The public embedding entry points are `start_sync` and `start_async` on an Arc
prepared block. They require an explicit CleanupSupervisor. The old driver bridges
are removed: adapters implement the complete DriverContext entry point. Low-level
CoreWasmBlock run methods are internal/testing machinery rather than public
alternatives that lose host-state ownership.

An ExecutionOwner owns its service Owner and result receiver. Admission failures
return the supplied host. A supervised task executes and sends ExecutionOutcome,
which holds the execution result, host, panic marker and final usage separately.
Waiting borrows the owner; timeout or dropped wait does not consume the outcome.
Dropping the owner signals cancellation, while the service supervisor retains
unfinished tasks. Joining returns host state once; subsequent joins fail explicitly.

For synchronous imports, acceptance is entry into the host method after checking
cancellation/deadline. For async imports, the host future is polled to completion
once entered. Cancellation prevents subsequent imports and interrupts guest CPU
execution; it does not drop an outstanding host future. A stuck host therefore
remains inspectable and charged. Hosts that spawn work outside their own operation
must supervise it separately. Runtime/namespace operations already have their own
request cancellation and provider owners.

Host panics are caught around guest execution so the Wasmtime store can release
its host data. The recovered state is marked potentially inconsistent. Process
abort and executor destruction are outside recovery guarantees; keep the executor
and supervisor alive until joining. No native host code can be forcibly reclaimed.

Engine cadence is configured once; ExecutionPolicy contains no epoch interval.
Fuel, optional absolute deadline, growth-denial policy and a per-run memory ceiling
apply to instantiation and execution. The default denial policy traps, even when
Wasm ignores memory.grow's result. ReturnFailure explicitly requests Wasm-style
failed growth. This is a linear-memory contract, not a host RSS cap. Manifest
inspection has separate fuel and memory bounds. Cleanup waits never redefine the
execution deadline or return state while host effects can still mutate it.

The Wasm/Value encodings are unchanged. Canonical server read responses require an
explicit boolean presence marker; unmarked historical responses are rejected.
This semantic requirement is part of the 2026-09-14 specification candidate, paired
with 0.4 runtime and SDK packages. Older artifacts must be rebuilt or explicitly
adapted; matching import signatures alone do not prove compatibility.
