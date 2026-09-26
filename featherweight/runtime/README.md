# featherweight-runtime

A strawman [Isotope](https://github.com/StructFS/structfs/tree/main/isotope/spec)
runtime: blocks are pico-processes whose entire world is
[StructFS](https://github.com/StructFS/structfs) reads and writes.

- **Blocks** run native Rust (`NativeBlock`) or core-binding wasm
  (`CoreWasmBlock`) against a per-block **namespace**: unwired paths are
  denied, reads and writes alike. Filesystem or network access is
  granted by wiring, never ambient.
- **`/iso/`** is the syscall surface — identity, env, time, randomness,
  stdio, logging, timers, shutdown, capability discovery and the instance
  budget — served as ordinary store paths and described by `iso/meta`.
- **The server protocol** makes every block a store: operations routed
  to a block become `{op, path, data, respond_to}` requests read from
  `iso/server/requests`; the block's response write resolves the
  caller's parked operation.
- **Assemblies** compose blocks with capability wiring; nested assembly
  definitions instantiate recursively, the public block starts eagerly,
  everything else lazily on first access.
- **Metering** governs guest execution: an optional per-run fuel cap, and
  engine-wide epoch interruption, so immediate shutdown can stop a
  spinning guest while parked store reads stay untouched.

## Starting guest code

There are exactly two supported ways to run guest code:

1. **Assemblies** — `Runtime::instantiate(&def, imports, base_dir)` wires
   blocks into namespaces and starts them. Code reaches an assembly as a
   `builtin:` native block, a `.wasm` file (loaded by the built-in core-wasm
   loader or a registered adapter loader), or an artifact identifier
   registered ahead of time with `RuntimeConfig::register_core_artifact` /
   `register_artifact`. `Runtime::management_store` and a block's granted
   `iso/proc` are the store-surface form of the same call.
2. **Owned embedding** — `CoreWasmBlock::start_sync` / `start_async` run one
   prepared core-wasm artifact over a host store you own, under an explicit
   `CleanupSupervisor` and `ExecutionPolicy`, and give the host store back
   through `ExecutionOwner::join`.

Everything else (the drivers' `execute` entry point, the loaders) is an
extension point the runtime calls, not a way for an embedder to start code.

## Configuring a runtime

A `RuntimeConfig` holds everything a runtime is configured with — the Tokio
handle, the per-operation timeout, the shared call budget, an execution
scope, the log sink and stdio provider, metering, transcripts, determinism,
the session log, and the registered builtins, loaders and artifacts.
`Runtime::new(config)` freezes it: a running runtime cannot be reconfigured,
so every block of every assembly it instantiates sees the same settings.

```rust,no_run
use std::collections::HashMap;
use featherweight_runtime::{register_builtins, AssemblyDef, Runtime, RuntimeConfig};

# async fn demo() -> featherweight_runtime::Result<()> {
let mut config = RuntimeConfig::new(tokio::runtime::Handle::current());
register_builtins(&mut config);
let runtime = Runtime::new(config);

let def = AssemblyDef::from_str(
    r#"{"assembly": "demo", "blocks": {"kv": "builtin:kv"}, "public": "kv"}"#,
)?;
let assembly = runtime.instantiate(&def, HashMap::new(), ".".as_ref())?;

use structfs_core_store::{path, Value};
assembly.write(path!("users/alice"), Value::from("hi")).await?;
assert_eq!(
    assembly.read(path!("users/alice")).await?,
    Some(Value::from("hi")),
);
# Ok(()) }
```

A failed `instantiate` leaves nothing behind: blocks it already created,
nested assemblies and started blocks included, are shut down and
deregistered (`Runtime::registered_blocks()` returns to its prior value).

## The wasm binding

The runtime core speaks exactly one wasm binding — the
[core-wasm binding](https://github.com/StructFS/structfs/blob/main/isotope/spec/11-core-wasm-binding.md):
two imports in the `structfs` module, no bindgen or component tooling.
Plain `cargo build --target wasm32-unknown-unknown` output runs
directly. Typed store errors cross it as the spec 11 status codes
(`protocol::ErrorKind` is the single table behind the status codes, the
server-protocol error types and the transcript/session labels).

Other artifact kinds attach as adapters through
`RuntimeConfig::register_loader` — for example `featherweight-component`
teaches the runtime to run WIT component-model artifacts as blocks.
Adapters configure, tick and arm their Wasmtime engines with the runtime's
hidden `adapter` hooks so metering and cancellation match the core binding;
those hooks name Wasmtime types and are outside the semver contract (an
adapter must build against the same Wasmtime release). WASI is a shim above
the Block ABI (`featherweight-wasi`), never a runtime dependency.

## Execution and host providers

Core Wasm runs through Wasmtime async fibers. Mailbox waits and detached
provider calls suspend the guest without retaining a blocking worker. Fuel
yields keep the executor responsive even without a configured fuel cap;
epoch interruption controls cancellation of spinning guest code.
Native blocks use the blocking pool; blocking host stdin is read there too.

Use `async_host_store` for `DetachedStore` providers: their methods must
return promptly, and their futures perform the waiting after the provider
lock is released. `host_store` wraps synchronous providers, dispatching
operations to the blocking pool; `service_host_store` mounts a native
service. `HostStore` itself is asynchronous (`read(..).await`,
`write(..).await`), and the runtime drives every host store it owns —
imports, transcripts, the session log — without blocking a worker thread.
The synchronous `Namespace` `Reader`/`Writer` facade exists for native
blocks, which run on the blocking pool. Providers remain responsible for
their I/O cancellation and deadlines.

Core-binding transfers validate memory ranges before copying and reject
individual payloads larger than 64 MiB. This is a host transfer limit, not
a limit on guest memory or provider-side encoding allocations.

## Metering

`Metering` is per run and carries one knob: `fuel` (`Metering::with_fuel`
caps a run at that many units of Wasmtime fuel; the default counts fuel but
does not cap it). Both the core-wasm binding and the component adapter
honour it.

Epoch interruption is an engine concern and always on: one ticker per
engine advances the epoch (every 10 ms by default), and at each tick a
running guest checks its cancellation and deadline. There is no per-run
epoch setting — a run cannot change a shared engine's ticker, and disabling
interruption would leave a spinning guest unstoppable. Change the cadence
per engine with `CoreWasmEngine::with_epoch_interval` (any positive
interval) or `ComponentEngine::with_epoch_interval` in the adapter.

## Prepared artifacts and fresh sessions

`CoreWasmEngine::new(compile_parallelism)` creates a shared engine with one
10 ms epoch ticker, bounded compilation concurrency, up to 10,000 active
guest stores, and a 64 MiB linear-memory limit per store. `with_limits`
configures the store count and memory limit. Prepared stores also allow one
memory, one table (up to 100,000 elements), and one instance. These are host
policy defaults, not new binding requirements. Memory limits also apply to
manifest inspection. Hosts must budget aggregate memory separately.

Call `engine.prepare(bytes).await` once for a verified artifact and retain the
returned `CoreWasmBlock` in an `Arc`. Preparation compiles once and retrieves a
fuel-bounded JSON manifest from the same module. Register that block with
`config.register_core_artifact(identifier, block)`; assembly definitions can
refer to the identifier without filesystem lookup. Each execution gets fresh
memory, globals, fuel and providers. Keep the engine's Tokio executor alive
through all its sessions: its ticker, compilations and synchronous runs use it.

`.wasm` files named in assembly definitions are loaded by the built-in loader,
which shares one engine (and one ticker) across every artifact a runtime
loads. That engine has the `CoreWasmEngine::new` defaults: at most 10,000
concurrent core-wasm runs per runtime, 64 MiB of linear memory each. A block
that starts while every slot is taken waits — cancellably, within its
execution scope — until a run ends; `CoreWasmEngine::available_sessions`
shows the headroom. Pass `RuntimeConfig::with_core_engine` to choose other
limits or to share an engine (and its cap) across runtimes.

A host can share prepared artifacts across separate `Runtime` instances to
isolate request policy. `AssemblyInstance::shutdown(timeout)` works to one
deadline, `timeout` from the call, for the whole assembly tree: blocks are
asked to stop gracefully, those still running at the halfway point are
escalated to immediate shutdown, and driver tasks and provider cleanup are
joined until the deadline. Whatever has not been joined by then is reported
in the `ShutdownReport` rather than waited for — call `shutdown` again, and
keep host reservations until a report is `complete()`. `Duration::ZERO`
therefore escalates at once and joins only work that has already finished.
Noncooperative native work remains registered; inspect
`Runtime::registered_blocks()` rather than assuming it was released. Explicit
shutdown is required. Dropping a request handle alone does not close a session.
Cancellation interrupts async guest execution, including parked imports and
waiting for a store slot. Providers must make dropping their futures safe;
already-dispatched synchronous work cannot be forcibly cancelled this way.

Releasing a spawn handle (`iso/proc/outstanding/{id}` or the management
store) shuts the child assembly down on a task supervised by the runtime's
cleanup supervisor (`Runtime::cleanup_supervisor`); an unclean shutdown is
counted as a failure in its reports.

For bounded embedding, reserve the entire assembly with
`engine.reserve_session(block_count)` and bind each prepared artifact with
`artifact.in_session(reservation.clone())`. Include lazy dependencies in the
count. Admission fails before execution if the whole reservation is unavailable.
Dynamic spawning needs additional admission.

`CallBudget` limits outstanding routed calls and logical payload bytes globally
and per block; overload fails immediately. Dropping or timing out a routed call
removes its queue entry, response identity and budget charge. Separate host-only
IDs prevent quota collisions across runtimes sharing transcript identities.

`set_limits` updates retain live charges, grandfather existing work, and apply
the new ceilings to subsequent admissions. Zero closes admission. Raising a
limit permits new work immediately. Call budgets support
`global.child(tenant_limits).child(request_limits)`; give the request child to
`RuntimeConfig::with_call_budget` and retain its handle for live updates and
inspection. Every accepted call charges all ancestors, and cancellation refunds
them all. `usage()` reports current occupancy; `metrics()` reports cumulative
admitted calls/logical bytes, rejected calls, and peak occupancy. Rejections are
counted at each budget consulted, not at ancestors skipped by an earlier
rejection. These are admission measurements, not CPU, fuel consumption, or
process RSS. Fuel and execution deadlines remain configured before execution,
and guest linear-memory ceilings remain engine settings.

`RuntimeConfig::with_execution_scope` applies one absolute deadline and
cancellation token across a fresh request runtime's blocks, routed calls and
providers. An HTTP owner should cancel that scope on disconnect and retain a
cleanup task until shutdown completes.

These APIs do not account for total process RSS, compiler allocations or HTTP
connection state. Events and retained replies have separate budgets described
below. Admission limits are ceilings, not a fair-scheduling guarantee. Artifact
cache eviction and durable provider effects remain host responsibilities.

The [capacity harness](tests/capacity.rs) exercises two rounds of fresh sessions
with simultaneous provider waits and checks response correctness, memory
isolation, registration cleanup and provider release. It uses 128 sessions in
normal tests. Run the larger experiment with:

```sh
FW_CAPACITY=10000 cargo test -p featherweight-runtime --test capacity --locked --offline -- --nocapture
```

See [recorded measurements](../../docs/history/featherweight-migration-progress.md).

## Status

This is the reference strawman for the Isotope spec, not a production
OS: JSON/CBOR/FlexBuffers transports are supported at the block
boundary, but there is no hash verification, no registries, no restart
policy. Deadlock detection exists only under `Determinism::Simulation`,
where the seeded scheduler detects internal dependency cycles; it does
not bound arbitrary host I/O, and `Live`/`Seeded` runs have no deadlock
detection. The
[spec](https://github.com/StructFS/structfs/tree/main/isotope/spec) is
the contract; this crate is the working model of it.

## External embedding

Prepare code in the embedding host, then call `RuntimeConfig::register_artifact`
with an `Arc<dyn WasmBlockDriver>`. Implement `execute(DriverContext)` for
asynchronous execution; the context supplies namespace, cancellation, metering,
the execution scope, the shared call budget and the instance meter. The built-in
core driver uses the same entry point. A synchronous adapter must do its own
blocking inside `execute`; the runtime never bridges it.

For persistent services, keep one assembly and create an `assembly.request(...)`
owner per client. Its deadline/cancellation and local call budget do not stop
other clients. Server adapters can use `namespace.request_cancellation` to cancel
request-owned provider waits. Cancellation cannot undo committed effects.
Always call `shutdown` and inspect its `ShutdownReport` before refunding instance
reservations. Driver panics become terminal failures; noncooperative native code
is reported as remaining work.

`AssemblyInstance::public_cell()` and `cell(name)` return a read-only
`BlockView`: identity, state, last error, exit code, declared interface,
pending counts, shutdown status and `usage()`. Lifecycle transitions, mailbox
traffic and responses belong to the runtime; nothing reachable from a view
changes the block. `usage()` reports Wasmtime fuel, current/peak
linear-memory bytes, configured execution limits and optional adapter counters
with explicit units. Core-Wasm samples at imports, epoch yields and exit;
missing measurements are absent. Joined execution has zero current memory,
while peaks and consumption remain available. These are not process RSS or
request-attributed CPU measurements. `iso/execution/budget` exposes policy
revisions and accounting read-only; `iso/capabilities` lists granted mount
prefixes. Network, binary console and configuration providers should be wired
outside `iso/`.

Signals and registered timers share a per-block event budget
(`BlockView::event_budget`), defaulting to 256 events and 1 MiB logical
payload. Timer reservations last until cancellation, event consumption, or the
end of the block's run, which cancels its timers. Retained response payloads are
bounded until consumption; oversized responses become typed overload errors.
`AssemblyInstance::signal` returns false when the event budget is full.

The independent consumer fixture lives in `tests/embedding` in the repository.
`python3.12 scripts/check-featherweight-release.py` builds actual Cargo archives,
runs that consumer and runtime/handle tests against the extracted packages, builds
the guest SDK feature modes for wasm32, and checks documentation. It never
publishes. See Isotope spec 13 for the detailed embedding and service contracts.

## Capacity and cleanup admission

The 10,000-session capacity harness explicitly reserves 40,000 routed-call slots:
parked sessions retain both the external caller and a nested provider call. The
runtime default remains 16,384 calls; increase it explicitly for larger workloads.
The harness checks zero retained call/byte charges and complete shutdown reports.

Cleanup supervisors retain at most their configured owner capacity, reclaiming
quiescent owners when admission needs space. This avoids scanning every live owner
on each new instance. Quiescent owner records may remain visible until reclamation;
unfinished work and unacknowledged failures always retain their capacity slots.
See the [release measurements](https://github.com/StructFS/structfs/blob/main/docs/history/release-validation-2026-09-12.md)
for the workload, observed memory and timing, and limits of those observations.

## Complete HTTP lifecycle example

Run `cargo run --manifest-path tests/embedding/Cargo.toml --example cancelled_http`
from the repository root. The [example](../../tests/embedding/examples/cancelled_http.rs)
and its [implementation](../../tests/embedding/src/lifecycle.rs) reuse one prepared
module across real loopback HTTP disconnects. The external allocator returns
accepted results after cancellation; `OwnerHandle::open` retains them until their
release callbacks join the actual producer, independently of public handle aliases.
Incomplete or failed cleanup retains the engine reservation and supervisor slot;
the example acknowledges a lost release reply only after checking actual termination.
The same guest also demonstrates a nonzero exit without a required trap diagnostic.

The HTTP exchange and allocator are deterministic fixtures. Adapt the lifecycle
ownership to your HTTP framework and external protocol. Keep the executor alive
until engine ticking and all retained cleanup are finished; a grace timeout is not
proof of resource release. The release gate runs this example against extracted
Cargo archives. See the [migration guide](../../docs/migration-0.4.md).

Assembly standard sections and block fields are strict. Unknown fields fail except
for ignored `x-` extension metadata. `config` and `failure` keys must name blocks;
per-block configuration values remain unrestricted application data. YAML and
JSON definitions parse to the same value.

## Recoverable prepared hosting

`CoreWasmEngine::prepare` produces reusable code. Wrap it in `Arc` and use
`start_sync` or `start_async` with an explicit `CleanupSupervisor` and
`ExecutionPolicy` (built with `ExecutionPolicy::default().with_fuel(..)`,
`with_memory_bytes`, `with_deadline`, `with_growth_failure`). Join the
`ExecutionOwner` to obtain both the execution result and the original host
state. Cancellation retains accepted effects until they finish; bounded waits
leave the owner joinable. Dropped owners leave unfinished work under the
supervisor. A host panic is reported as `host_panicked` with the host returned;
a run whose blocking worker never started returns the untouched host with
`RuntimeError::ExecutionLost`. A second join fails with
`RuntimeError::AlreadyJoined`.

The prepared_hosting example measures 100 fresh sync and async runs over one
prepared module. See [0.4 migration](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md)
for policy defaults, panic/recovery limits and the retained provider interfaces.
