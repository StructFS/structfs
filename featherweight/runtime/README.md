# featherweight-runtime

A strawman [Isotope](https://github.com/StructFS/structfs/tree/main/isotope/spec)
runtime: blocks are pico-processes whose entire world is
[StructFS](https://github.com/StructFS/structfs) reads and writes.

- **Blocks** run native Rust (`NativeBlock`) or core-binding wasm
  (`CoreWasmBlock`) against a per-block **namespace**: unwired paths are
  denied, reads and writes alike. Filesystem or network access is
  granted by wiring, never ambient.
- **`/iso/`** is the syscall surface — identity, env, time, randomness,
  stdio, logging, timers, shutdown — served as ordinary store paths.
- **The server protocol** makes every block a store: operations routed
  to a block become `{op, path, data, respond_to}` requests read from
  `iso/server/requests`; the block's response write resolves the
  caller's parked operation.
- **Assemblies** compose blocks with capability wiring; nested assembly
  definitions instantiate recursively, the public block starts eagerly,
  everything else lazily on first access.
- **Metering** governs guest execution: optional fuel caps and epoch
  interruption, so immediate shutdown can stop a spinning guest while
  parked store reads stay untouched.

## The wasm binding

The runtime core speaks exactly one wasm binding — the
[core-wasm binding](https://github.com/StructFS/structfs/blob/main/isotope/spec/11-core-wasm-binding.md):
two imports in the `structfs` module, no bindgen or component tooling.
Plain `cargo build --target wasm32-unknown-unknown` output runs
directly.

Other artifact kinds attach as adapters through
`Runtime::register_loader` — for example `featherweight-component`
teaches the runtime to run WIT component-model artifacts as blocks.
WASI is a shim above the Block ABI (`featherweight-wasi`), never a
runtime dependency.

## Execution and host providers

Core Wasm runs through Wasmtime async fibers. Mailbox waits and detached
provider calls suspend the guest without retaining a blocking worker. Fuel
yields keep the executor responsive even without a configured fuel cap;
epoch interruption still controls cancellation of spinning guest code.
Native blocks and synchronous binding adapters use the blocking pool.

Use `async_host_store` for `DetachedStore` providers: their methods must
return promptly, and their futures perform the waiting after the provider
lock is released. `host_store` wraps synchronous providers, dispatching
operations to the blocking pool. The synchronous `Namespace` and `HostStore`
interfaces must be called from blocking threads when they bridge async work.
Providers remain responsible for their I/O cancellation and deadlines.

Core-binding transfers validate memory ranges before copying and reject
individual payloads larger than 64 MiB. This is a host transfer limit, not
a limit on guest memory or provider-side encoding allocations.

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
fuel-bounded JSON manifest from the same module. Register that block using
`runtime.register_core_artifact(identifier, block)`; assembly definitions can
refer to the identifier without filesystem lookup. Each execution gets fresh
memory, globals, fuel and providers. Prepared artifacts require async execution;
custom epoch intervals other than 10 ms are rejected. Disabling epochs remains
supported. Keep the engine's Tokio executor alive through all its sessions.

A host can share prepared artifacts across separate `Runtime` instances to
isolate request policy. `AssemblyInstance::shutdown` uses one grace deadline
for its assembly tree, escalates, joins driver tasks, and removes stopped
registrations. Noncooperative native work remains registered; inspect
`Runtime::registered_blocks()` rather than assuming it was released. Explicit
shutdown is required. Dropping a request handle alone does not close a session.
Cancellation interrupts async guest execution, including parked imports and
waiting for a store slot. Providers must make dropping their futures safe;
already-dispatched synchronous work cannot be forcibly cancelled this way.

For bounded embedding, reserve the entire assembly with
`engine.reserve_session(block_count)` and bind each prepared artifact with
`artifact.in_session(reservation.clone())`. Include lazy dependencies in the
count. Admission fails before execution if the whole reservation is unavailable.
Dynamic spawning needs additional admission.

`SessionBudget` reserves request count, guest slots and worst-case guest
linear memory, with a per-tenant ceiling. Hold its permit until teardown.
`CallBudget` limits outstanding routed calls and logical payload bytes globally
and per block; overload fails immediately. Dropping or timing out a routed call
removes its queue entry, response identity and budget charge. Separate host-only
IDs prevent quota collisions across runtimes sharing transcript identities.

Both budgets expose `set_limits`: updates retain live charges, grandfather
existing work, and apply the new ceilings to subsequent admissions. Zero closes
admission. Raising a limit permits new work immediately. Call budgets support
`global.child(tenant_limits).child(request_limits)`; give the request child to
`Runtime::with_call_budget` and retain its handle for live updates and inspection.
Every accepted call charges all ancestors, and cancellation refunds them all.
`usage()` reports current occupancy; `metrics()` reports cumulative admitted
calls/logical bytes, rejected calls, and peak occupancy. Rejections are counted
at each budget consulted, not at ancestors skipped by an earlier rejection.
These are admission measurements, not CPU, fuel consumption, or process RSS.
Fuel and execution deadlines remain configured before execution, and guest
linear-memory ceilings remain engine settings.

`Runtime::with_execution_scope` applies one absolute deadline and cancellation
token across a fresh request runtime's blocks, routed calls and providers. An
HTTP owner should cancel that scope on disconnect and retain a cleanup task
until shutdown completes. The Appiware native listener demonstrates this pattern.

These APIs do not yet bound all timer/signal queues, logs, response retention,
compiler allocations or HTTP connection state. Admission limits are ceilings,
not a fair-scheduling guarantee. Artifact cache eviction and durable provider
effects remain host responsibilities.

The [capacity harness](tests/capacity.rs) exercises two rounds of fresh sessions
with simultaneous provider waits and checks response correctness, memory
isolation, registration cleanup and provider release. It uses 128 sessions in
normal tests. Run the larger experiment with:

```sh
FW_CAPACITY=10000 cargo test -p featherweight-runtime --test capacity --locked --offline -- --nocapture
```

See [recorded measurements](../../docs/featherweight-migration-progress.md).

## Example

```rust,no_run
use std::collections::HashMap;
use featherweight_runtime::{register_builtins, AssemblyDef, Runtime};

# async fn demo() -> featherweight_runtime::Result<()> {
let mut runtime = Runtime::new();
register_builtins(&mut runtime);

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

## Status

This is the reference strawman for the Isotope spec, not a production
OS: JSON/CBOR/FlexBuffers transports are supported at the block
boundary, but there is no hash verification, no registries, no restart
policy. Seeded simulation detects internal dependency deadlocks; it does
not bound arbitrary host I/O. The
[spec](https://github.com/StructFS/structfs/tree/main/isotope/spec) is
the contract; this crate is the working model of it.

## External embedding (next release)

Prepare code in the embedding host, then call `Runtime::register_artifact` with
an `Arc<dyn WasmBlockDriver>`. Implement `execute(DriverContext)` for asynchronous
execution; the context supplies namespace, cancellation, policy and instance
metering. The built-in core driver uses the same entry point. Existing loader
and synchronous driver adapters remain supported.

For persistent services, keep one assembly and create an `assembly.request(...)`
owner per client. Its deadline/cancellation and local call budget do not stop
other clients. Server adapters can use `namespace.request_cancellation` to cancel
request-owned provider waits. Cancellation cannot undo committed effects.
Always call `shutdown` and inspect its `ShutdownReport` before refunding instance
reservations. Driver panics become terminal failures; noncooperative native code
is reported as remaining work.

`cell.usage.snapshot()` reports Wasmtime fuel, current/peak linear-memory bytes,
configured execution limits and optional adapter counters with explicit units.
Core-Wasm samples at imports, epoch yields and exit; missing measurements are
absent. Joined execution has zero current memory, while peaks and consumption
remain available. These are not process RSS or request-attributed CPU measurements.
`iso/execution/budget` exposes policy revisions and accounting read-only;
`iso/capabilities` lists granted mount prefixes. Network, binary console and
configuration providers should be wired outside `iso/`.

Signals and registered timers share `cell.events`, a live budget defaulting to
256 events and 1 MiB logical payload. Timer reservations last until cancellation
or event consumption. `cell.replies` bounds retained response payloads until
consumption; oversized responses become typed overload errors. `deliver_signal`/`deliver_timer` now return typed admission
errors. Unowned raw mailbox enqueue is no longer a public embedding API; use
assembly operations so cancellation cleans up correlations.

`DriverCapabilities` and `DriverControl` let an adapter expose resumable execution
or checkpoint controls without changing the core binding. The adapter owns safe
points and provider-state validation; no universal snapshot support is implied.

The independent consumer fixture lives in `tests/embedding` in the repository.
`python3.12 scripts/check-featherweight-release.py` builds actual Cargo archives,
runs that consumer and runtime/handle tests against the extracted packages, builds
both guest SDK feature modes for wasm32, and checks documentation. It never
publishes. See Isotope spec 13 for the detailed embedding and service contracts.
