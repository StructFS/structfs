# Application profiles and migration fixtures

Status: implemented for the next 0.2.0 release; not published.

This implements P0-E in the [application substrate plan](2026-09-11-application-substrate-and-value-ir.md).
The migration target combines Value v1, owned async services, Featherweight
execution ownership, revisioned state, and independent application profiles.
Ox retains its application reducers, renderers, ledger and authority policy.

## Shipped surface

`structfs-profiles` adds portable declarations, interactive input/status, operation
status, approval/process request schemas and validated durability acknowledgments.
Its default `host` feature adds pure discovery, bounded headless sessions and owned
operation handles. Disable defaults for guest schemas. The `structfs` facade adds
an optional `profiles` feature, included in `full`.

Isotope 03, 06, 07, 11 and 13 align discovery, Value fidelity, response presence,
status categories and ownership. [Isotope 14](../../isotope/spec/14-capability-profiles.md)
is the application-profile contract. Profile versions are independent of crate
versions; there are still exactly two core-Wasm imports.

The guest SDK's `profiles` feature reads discovery and reexports portable schemas.
`read_typed`/`write_typed` retain ABI status and diagnostics. Value/profile helpers
preserve host categories, local codec errors and protocol-validation failures;
legacy diagnostic-string wrappers remain available. Explicit server response
presence distinguishes Null from absence; unmarked Null responses retain their
legacy meaning. Servers previously calling `ok_value(Null)` for absence must use
`ok_absent()`.

## Independent application evidence

The packaging gate extracts actual Cargo archives, strips source path dependencies
from four separate consumers, and resolves local versions only through patches to
extracted archives. There is no Ox dependency. The original embedding consumer
remains; three application consumers add the following evidence.

| Consumer | Executed scenarios |
| --- | --- |
| reactive-screen | Current-thread native reducer and actual core guest orchestration produce identical projections; equal labels under distinct mount prefixes; repeated input retained; queue rejection leaves sequence unchanged; pending commit before immediate effect; stale generation rejected despite ignored cancellation; expired-history resync; separate processed/rendered acknowledgments; credential-presence projection; closing one installation preserves its peer |
| streaming-gateway | One prepared artifact reused in fresh instances; bounded fake upstream/response; slow reader and final-chunk EOF; admission and size rejection; trap/fuel failure; disconnect before and after open; lost open-result delivery; timeout refund; noncooperative work remains charged until joined; cleanup needs no guest final writes |
| conversation-service | Persistent owned host service across fresh guest turns; operation-specific approvals; observer disconnect independent of turn; cancellation and service close join work; file-sync acknowledgment before event; duplicate-ID content contract across restart; old state epoch rejected; configuration snapshot does not imply persistence; fake process grants, duplex I/O, exit result, cancellation and join |

The screen uses one host reducer called through the same Service interface from
native code or WAT. It does not compile a Horns renderer into Wasm. Credential
projection demonstrates the public presence-only shape, not a production secret
store. The gateway upstream is a bounded fake: HTTP/2, TLS, sockets and production
proxy adapters are not certified. It prints workload elapsed microseconds and peak
guest linear-memory bytes separately. These are smoke observations, not statistical
benchmarks, CPU time, total RSS or Wasmtime compiler memory.

The journal uses append plus `sync_all`. Recovery assumes complete records; it does
not certify torn-write recovery, directory durability, atomic replacement,
multi-process concurrency or database transactions. The fake process neither spawns
nor sandboxes an OS process. Ox's own remount, ledger, approval, gateway and worker
tests remain required migration acceptance tests.

## Host and toolchain matrix

| Scope | Validated target / limit |
| --- | --- |
| Native workspace and external consumers | Rust 1.96.0, aarch64-apple-darwin; current-thread screen and multithreaded runtime tests |
| Portable guest SDK | wasm32-unknown-unknown builds: default, no defaults, value-codecs, state, profiles |
| Portable profiles | Native tests and wasm32-unknown-unknown build with no defaults |
| Browser-compatible host code | TypeScript check and Node 25.9 suite; shared interactive corpus and bigint u64 boundaries |
| Actual browser engine / worker residency | Not newly exercised or certified by these headless protocol tests |
| Linux / Windows native adapters | Not exercised by this work |
| Lower MSRV | No new minimum-version promise; 1.96.0 is the tested release baseline |

Browser-compatible TypeScript and Rust validate the same input corpus, including
unknown fields/variants. Browser integration still owns the presentation surface
and renderer; the headless model alone provides neither worker residency nor DOM
interaction.

## Reproducible gates

From the repository root, with cached dependencies, Python 3.11+ and the Wasm target:

```sh
cargo test --workspace --all-features --locked --offline
cargo clippy --workspace --all-features --all-targets --locked --offline -- -D warnings
python3.12 scripts/check-featherweight-release.py
```

The archive gate checks every extracted publishable crate, runs runtime/service/
value/state/profile tests and all four consumers, runs strict consumer Clippy,
builds guest feature combinations and portable profiles, and builds docs with
warnings denied. It does not publish. In `featherweight/host/browser`, run
`npm run check` and `npm test`. The corrected test glob selects TypeScript test files
under Node 25.

Validation on the host above: 1,329 workspace tests passed (25 pre-existing ignored
examples/targets), 662 archive test executions passed (15 ignored), and 21 Node
browser-host tests passed. Strict workspace/consumer Clippy, TypeScript checking,
portable feature builds and documentation checks passed. The workspace HTTP tests
required an unrestricted rerun because the macOS sandbox blocks SystemConfiguration;
all seven affected tests passed on that rerun. Counts include Rust doctests and the
archive gate's separate portable-profile test invocation.

## Next priorities

P0-E supplies a reviewable migration contract and executable application examples.
P1 should add the production adapters needed by Ox:

1. Gateway upstream/response adapters with socket-level disconnect/deadline tests
   and measured admission, latency and memory behavior under load.
2. Durable configuration/ledger adapters with crash recovery and explicit fsync,
   directory-sync, duplicate-ID and transaction guarantees.
3. A process provider with executable/environment/workspace grants, bounded stdio,
   exit/termination escalation, and platform-specific isolation tests.
4. Browser/Horns presentation integration with worker residency and cross-engine
   input/render acknowledgment tests.

RON remains a candidate authoring syntax over Value. It replaces neither the IR,
state, ownership, transport ABI nor durability contracts. Add it after the migration
adapters, with an explicit supported-shape and fidelity corpus.
