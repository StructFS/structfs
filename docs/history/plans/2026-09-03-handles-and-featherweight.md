# Handles, Streaming, and the Featherweight Isotope Runtime

Design for two deliverables:

1. `structfs-handles`: the handle/streaming primitive, designed against the
   four hand-rolled handle stores in ox-gateway (`wire_store`,
   `telemetry_store`, `upstream_store`, `completion_broker`).
2. A rebuilt Featherweight: an Isotope runtime that loads assemblies, wires
   per-block namespaces, serves the `/iso/` system store, and runs the
   server protocol — with a native demo shell.

## Part 1: `structfs-handles`

### What the gateway stores taught us

Every one of the four stores duplicates the same five mechanisms, and two of
them are hazards when hand-written:

| Mechanism | Hazard |
|---|---|
| `outstanding/{id}` path parsing + id minting | boilerplate |
| "cannot overwrite a handle; write Null to GC" | boilerplate |
| park-until-predicate over `tokio::sync::Notify` | **lost-wakeup** unless the notified future is created before the state check |
| event tail at `events/from/{seq}` with terminality on a separate path | **close-out race**: events landing between the tail read and the terminal read |
| cancellation on GC | reads must fail, writes must stay open so teardown lands |

### The primitives

**`Gate`** — park until a predicate holds. The enable-before-check ordering is
inside the primitive, so no store author writes the lost-wakeup bug again.
`wait_until(check)` and `wait_until_cancelled(check, token)`.

**`CancelToken`** — cancellation observable by parked reads. `cancel()` wakes
every parked `Gate` wait that carries the token; cancelled waits resolve to
`Error::DeadlineExceeded`-style cancellation errors. Cancellation applies to
reads only — writes proceed so teardown can land (the gateway's discovered
protocol rule).

**`TailLog<T>`** — an append-only event log with terminal state, read
atomically. `read_from(seq)` parks until there are events past `seq` *or* the
log is finished, and returns a `TailPage { items, next, done }` in one
operation. Terminality travels with the items, so the close-out race is
structurally impossible. Out-of-range cursors clamp instead of panicking.
The `Value` encoding is `{items, next, status: "open"|"done"}` — one read
gives a consumer everything it needs to either continue (`events/from/{next}`)
or stop.

**`HandleStore<P>`** — generic `outstanding/{id}` scaffolding over a
protocol implementation:

```rust
pub trait HandleProtocol: Send + Sync + 'static {
    type Handle: Send + Sync + 'static;
    fn open(&self, cx: HandleCx, request: Value) -> Result<Self::Handle, Error>;
    fn read(&self, handle: Arc<Self::Handle>, sub: Path) -> DetachedFuture<Option<Record>>;
    fn write(&self, handle: Arc<Self::Handle>, sub: Path, data: Record) -> DetachedFuture<Path>;
    fn close(&self, handle: Arc<Self::Handle>);
}
```

`HandleStore` owns: id minting, `outstanding/{id}[/sub]` routing, the
overwrite→`Conflict` rule, Null-write GC (cancel + close + unmap), root
listing, and post-GC reads returning `None`. `HandleCx` hands the protocol
its id and `CancelToken` so its parked reads are cancellable.

`HandleStore` implements the detached async traits (`DetachedReader`/
`DetachedWriter`); `SyncBridge` adapts any detached store to the sync
`Reader`/`Writer` traits over a tokio runtime handle, with the documented
contract that it must run on a blocking thread.

Not in scope here: rewriting `structfs-http`'s broker over `HandleStore`
(it should become a thin instantiation, but that's a follow-up port).

## Part 2: Featherweight as an Isotope runtime

Featherweight becomes a real (strawman) runtime for `isotope/spec`:

- **Assembly loader** (`assembly.rs`): JSON/YAML definitions → typed
  `AssemblyDef` (blocks, public, wiring, config, imports, failure modes).
  Nested assemblies (`*.json`/`*.yaml` block refs) instantiate recursively —
  the fractal property. `builtin:{name}` refs resolve native blocks from a
  registry; `*.wasm` refs load wasm components.
- **Wiring** (`wiring.rs`): longest-prefix, component-wise `WiringTable` with
  bidirectional rewrite (resolve caller→target, unresolve result paths back
  into the caller's namespace) and deny-by-default (`PermissionDenied`, not
  `NotFound` — unwired is a capability failure).
- **Namespace** (`namespace.rs`): per-block store routing `/iso` to the
  block's system store and wired prefixes to other blocks' stores.
- **`/iso/` store** (`iso.rs`): `self/{id,state,interface}`,
  `shutdown/{requested,mode,complete}`, `time/{now,monotonic,zone}`,
  `random/{uuid,int,bytes/{n}}`, `log/{level}`, and the server protocol
  surface `server/requests` (blocking, via `Gate`) and
  `server/requests/pending` (non-blocking batch).
- **Server protocol** (`server.rs`): a block-as-store gateway. Operations
  routed to a block become `{op, path, data, respond_to}` requests queued on
  its `/iso/server/requests`; the block's response write to `respond_to`
  resolves the caller's parked operation. Lazy start on first routed
  operation; graceful shutdown unblocks the request read with Null.
- **Lifecycle**: the spec's six states with one shared state cell per block
  (fixing the old three-disconnected-handles bug), fail-fast and isolate
  failure modes, graceful→immediate shutdown escalation with timeout.
- **Blocks**: native blocks (a `NativeBlock` trait run on blocking threads
  against a sync namespace facade) and wasm blocks (the existing WIT, with
  the manifest→codec loop finally closed via `serde-store`'s `JsonCodec`).
- **Demo shell** (`builtin:shell` + `fw` binary): an interactive native
  block whose commands (`read`, `write`, `ls`, `id`, `state`, `time`,
  `uuid`, `log`, …) execute against its own namespace, exercising the OS
  surface and wired services (`builtin:kv`, `builtin:echo`).

### Deliberate strawman simplifications

- Terminal I/O for the shell is direct stdin/stdout (the store model has no
  tty story; noted as an open spec question).
- Serialization declarations honor `application/json` only.
- No content-hash verification, no registries, no derived assemblies
  (`extends`), no restart failure policy (fail-fast + isolate only).
- Deadlock handling: none beyond the handle pattern (spec says runtimes
  should detect; strawman documents instead).
