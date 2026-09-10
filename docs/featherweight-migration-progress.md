# Featherweight embedding migration progress

Updated 2026-09-10. Implements part of the StructFS work described in
[Appiware's migration plan](../../appiware/docs/featherweight_migration.md).
A native Appiware echo listener now embeds the runtime; the existing Python/control-plane serving path has not switched.

## Implemented in the working tree

- Core Wasm execution uses Wasmtime async fibers and async namespace calls.
  Native drivers and synchronous binding adapters retain an explicit blocking
  execution path. Simulation identity follows async tasks and crosses the
  blocking adapter boundary explicitly.
- Detached host providers can be mounted with `async_host_store`. Provider
  locks are released before awaiting results. Spawn grants preserve async
  routing and return-path confinement.
- Cooperative fuel yields allow a current-thread executor to drive epoch
  cancellation. Async epoch ticker tasks are aborted when their guard drops.
- Core-binding imports and manifest result reads validate ranges before
  allocating copies. Individual transfers are limited to 64 MiB; result-record
  destinations are checked before invoking the guest allocator.

- `CoreWasmEngine` prepares a compiled module and bounded manifest once,
  shares one epoch ticker, bounds compilation and active guest stores, and
  enforces configurable per-store linear-memory limits. Runtime artifact
  registration accepts prepared code without a filesystem round trip.
- Async cancellation drops parked imports and admission waits. Explicit
  assembly shutdown joins tasks and removes stopped registrations under a
  shared tree deadline. Noncooperative native tasks remain visible.

## Verified

`cargo test -p featherweight-runtime --locked --offline` passes 116 tests
and one documentation test. Three existing fixture-regeneration/diagnostic
tests remain intentionally ignored. Coverage includes malformed import and
manifest lengths, async echo with one blocking worker, spinning-guest
cancellation on a current-thread executor, a detached provider read released
by a concurrent write, native composition and spawn grants, portable replay,
and the Wasm simulation seed sweep.

The full StructFS workspace also passes outside the macOS sandbox: 1,266
tests, zero failures, 23 existing ignored tests across 43 suites (including
documentation tests). Sandboxed HTTP-client initialization failed in macOS
SystemConfiguration; the unrestricted run passed. Clippy passes for the runtime,
guest SDK and component adapter, and for the native Appiware daemon.

## 10,000 concurrent request experiment

Run on arm64 macOS 26.4.1 with Rust 1.96.0, Wasmtime 48.0.1, the unoptimized
Cargo test profile, four Tokio workers and at most two blocking workers.
Command:

```sh
FW_CAPACITY=10000 cargo test -p featherweight-runtime --test capacity --locked --offline -- --nocapture
```

Two rounds each created 10,000 fresh single-block Wasm sessions from one
prepared module. Every session received one routed request and parked in a
detached host provider before any were released. All returned the expected
value; guest memory assertions detected neither manifest nor prior-run state.
The cancellation regression ran in the same test process.

| Measurement | Round 1 | Round 2 |
| --- | ---: | ---: |
| All 10,000 requests parked | 264 ms | 243 ms |
| Create, park, release, verify and clean up | 483 ms | 457 ms |
| Parked process RSS | 1,058,752 KiB | 1,059,392 KiB |
| Released process RSS | 259,136 KiB | 259,408 KiB |
| Parked / released process threads | 26 / 26 | 26 / 26 |
| Remaining block registrations | 0 | 0 |

RSS and threads were sampled with `ps`; these are point measurements, not
peak measurements. Allocator retention means RSS does not return to startup
levels. Provider reference counts returned to their baseline in both rounds.
This is a lightweight Wasm concurrency experiment, not HTTP throughput, Python
capacity, a long-running leak test, or a latency/fairness service guarantee.

## Request ownership and Appiware proof

Shared call budgets now bound outstanding request counts and logical payload
bytes globally and per block. A call owns queue/correlation cleanup on timeout
or drop. Host-only admission IDs distinguish cells in separate runtimes even
when their transcript IDs match. Session budgets reserve count, guest slots
and worst-case linear memory, with a per-tenant count ceiling. These ceilings
limit monopolization; they are not a fair-scheduling or latency guarantee.

`ExecutionScope` carries one absolute deadline and cancellation signal through
routed calls and namespace/provider operations. Runtime block cancellation
watches interrupt Wasm code as well. `CoreWasmEngine::reserve_session` reserves
all block slots up front, including lazy dependencies; `in_session` binds
prepared code to those slots. A gateway/worker test proves the lazy dependency
can start when no unreserved capacity remains.

The [Appiware native listener](../../appiware/daemon/native/README.md) uses
these APIs with a generated, SHA-256-pinned in-tree source snapshot and Cargo/
Bazel locks. Both repositories now use Wasmtime 48.0.1 for this path. A separate
SDK feature lets rebuilt applications use the native guest SDK without linking
the reference KV guest's exports.

Four native HTTP tests pass through Cargo and Bazel: binary requests through
1 MiB, methods and original request URI/query, repeated cookies, empty bodies,
rejected artifact digests, oversized-body rejection, and disconnect cleanup.
A real localhost TCP smoke also verifies binary echo and graceful shutdown.
The guest is optimized under both build systems; the debug guest exhausted
its fuel budget on the largest body.

The latest capacity experiment uses the new engine/admission implementation.
Parked RSS increased from approximately 804 MiB in the earlier experiment to
1.01 GiB, and released RSS from approximately 179 MiB to 253 MiB. The exact
contribution of Wasmtime versus admission changes has not been isolated.
Investigate this before setting production memory targets; two rounds are not
a long-running leak or resource-regression qualification.

## Remaining gates

This remains partial Phase 0/1 work. The Appiware proof is a single pinned
listener deployment; native control-plane configuration and artifact-service
resolution, bounded cache eviction/single-flight, real HTTP gateway/KV serving,
multi-deployment scheduling and sustained adversarial load tests remain.

Call quotas cover routed server requests, not every source of mailbox events:
signal/timer queues, logs, response retention and compiler/provider allocations
still need independent bounds. Session memory reservations cover guest linear
memory, not all host overhead. The admission APIs must be used consistently;
raw unreserved execution retains its existing behavior.

Explicit shutdown remains required. Noncooperative native work cannot be
forcibly stopped; async cancellation drops provider futures but cannot undo
effects or stop already-running synchronous providers. Unique transcript
identities remain in a runtime's bookkeeping; use a fresh runtime per execution
session. File-loaded unprepared guests retain their existing compilation path.

The Appiware P0 fixes, durable commit guarantees, Python/snapshot port, browser
convergence, retained-data import, self-hosting and cutover gates remain
outstanding as specified in the migration plan.
