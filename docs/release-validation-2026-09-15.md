# 0.4 candidate validation — 2026-09-15

The [holistic plan](../plans/02-coherent-contracts.md) is implemented for the
StructFS/Featherweight 0.4.0 candidate and Isotope 2026-09-14 specification candidate.
[Implementation decisions](design/2026-09-14-coherent-contracts.md) and
[migration](migration-0.4.md) describe the final contracts.

Source baseline: `c8a9b1527cebf4b7c6362971c9aea0c1cefc6aea` plus the uncommitted changes identified in
[the source manifest](release-validation-2026-09-15.json). This is a validated
working-tree candidate, not a release commit or tag. No packages were published.
The generated [registry record](release-status.md) verifies 0.4.0 as unpublished;
Namecode remains independently published at 0.1.1.

## Results

| Check | Result |
| --- | --- |
| `./scripts/quality_gates.sh` | Passed: format, strict Clippy, 1,368 tests; 24 ignored |
| Workspace coverage | 90.15% regions (threshold 90%); 89.61% lines |
| `scripts/check-release.sh` | Passed: workspace tests, strict Clippy, rustdoc, features, portable graphs, extracted packages, guests, browser |
| Extracted-package consumers | Embedding, conversation service, reactive screen, streaming gateway, portable consumer; passed |
| Packaged CLI | File loading, recording/replay/seek, invalid inputs and demo shell; passed |
| Guest builds | Rust and AssemblyScript package-gate builds and fixture checks passed |
| Browser host | 21 Node tests passed, including cross-host replay |
| Isotope site | Built from normative sources: 19 pages generated |
| Registry tooling | 13 Python tests passed; exact-version registry status generated |

The coverage command now uses `--all-features --locked`, matching the tests and
Clippy graph. Previously, coverage silently skipped async Serde contract tests.
The threshold and file exclusions are unchanged. Added CLI and shared-session
service tests exercise real behavior; the CLI test found and fixed preparation
outside an entered runtime when using Runtime::with_handle.

The release gate runs the CLI tests inside the extracted-package workspace as
well as checking every package's targets. It does not depend on sibling Ox sources.
Standalone consumer lockfiles retain their previous third-party dependency versions
while resolving the local 0.4 packages.

The [Ox letter audit](ox-letter-audit-2026-09-15.md) additionally records all 28
unchanged tests passing against an isolated copy of Ox's subscription sources
with its writer trait and suffix matcher replaced. The audit corrected poisoned
lock behavior across shared/detached interfaces and added queued cancellation,
post-cancellation dispatch, and async panic recovery regressions. Its source
provenance is included in the JSON record.

## Contract evidence

- `core-store/tests/coherent_contracts.rs`: explicit snapshot fidelity, conflicts,
  conventional Null writes and erased concurrent shared handles.
- `core-store/tests/pattern_allocations.rs`: repeated hits, misses, empty suffixes
  and extreme minima with zero allocations after fixture construction.
- `serde-store/tests/typed_parity.rs`: identical parsed/raw/absent/Null handling
  across synchronous, asynchronous and detached implicit reads.
- Portable consumer: renamed core dependency and facade macro expansion on Wasm.
- `state/tests/state.rs`: conventional Null assignment translates to internal
  Delete without removing the internal representation's ability to store Null.
- `runtime/tests/owned_execution.rs` and independent conversation consumer:
  prepared reuse and fresh memory, non-Clone recovery, traps and host panics,
  blocking and async cancellation, independent runs, incomplete joins and owner
  abandonment, growth policies, fuel/deadlines and failed instantiation.
- CLI file preparation reuses the inspected module and retains the configured
  runtime's epoch ticker instead of compiling again for execution.

## Scheduling sample

`cargo run -p featherweight-runtime --example prepared_hosting --offline` prepared
one module and executed 100 fresh instances in each hosting mode. One local debug
sample recorded 11,778 microseconds to prepare, 6,896 microseconds for 100 synchronous
runs, and 7,090 microseconds for 100 async runs. These are a reproducible smoke
sample, not a benchmark claim about Ox or a release performance threshold.

## Limits of the evidence

Owned host-state recovery is implemented for native core-Wasm hosting. The browser
and component adapters do not advertise that API or arbitrary ExecutionPolicy
support; their existing binding contracts remain separately tested. Blocking native
code and noncooperative async host operations cannot be forcibly reclaimed. Keep
supervision and the executor alive until joining; state recovered after a host
panic may be logically inconsistent. Coverage does not imply production durability,
OS process isolation, or certification on platforms not run here.

Native tests required execution outside the filesystem sandbox because macOS
system-configuration access failed inside it. Locked site dependencies required
registry access. Neither step published artifacts or contacted maintainers.

## Validated package archives

These SHA-256 hashes identify the artifacts extracted and tested by the final
release gate. Regenerating an archive can change its hash; verify again before
publication if its contents change.

| Archive | SHA-256 |
| --- | --- |
| featherweight-0.4.0.crate | `21aa93737fa5fd3ab2912aa57ffede3fe04c4b9617ca67fa67d852d1d3818e10` |
| featherweight-component-0.4.0.crate | `4c71062972981aaa007aab757779e5d68b69123cfbc3e5dacde34566ccbed03d` |
| featherweight-guest-0.4.0.crate | `42bda9b0f856bacd5e69e53d9c5ae71262ce524f122fa697cc95f2d4f4d74ddc` |
| featherweight-runtime-0.4.0.crate | `d2876a43ae8b880d96e80ff58fd70fb48d676d2fac864f3905f563f39cce4403` |
| featherweight-wasi-0.4.0.crate | `3e5aba5f331a9ffc1022e3bfe818c3f002238d6ddca2644f53e2f99e407460ef` |
| namecode-0.1.1.crate | `56a4dcbdf7ef06c57666beaf81e5d30faa0961253c9eef4c5871f3af13d87025` |
| structfs-0.4.0.crate | `a61b854e3c3f4e728cdd9ef8d2f102a5caff2622927faf8544fc8e9235f0068d` |
| structfs-core-store-0.4.0.crate | `2712dac6d5a22377a2e971372bbbf4686d90a5186693d59844c667226ed2c1a2` |
| structfs-handles-0.4.0.crate | `953cf8b523dec12180dfb1d88e3e771f365801be890ee1a296b27e652ad583fa` |
| structfs-http-0.4.0.crate | `e0128c1c02f0969bdce665d3fd658c9769c648068591cc0d59d68743993de15c` |
| structfs-json-store-0.4.0.crate | `58d9d03551fb2e61c5d85d6c4dad4880c19fc26d9e021d521f296557a4b94237` |
| structfs-ll-store-0.4.0.crate | `c8caada2bd60674333fab67dee2cf9d0dec90acff651b564a739f8a55677feb0` |
| structfs-path-macro-0.4.0.crate | `445ae032650dbfff32615fb789821dd73dc0e69ba786d5531fad33cc05905022` |
| structfs-path-validation-0.4.0.crate | `aca293498a7213d7f96efec3b3cf0aa0c5edd281380e3002ee874fa9f49b93ec` |
| structfs-profiles-0.4.0.crate | `b870f75271e7c267e3c0b30d836ffd4cf78a4e1722b88479cd35178fea97b278` |
| structfs-repl-0.4.0.crate | `a79474ec6e9f7483b6bee6860a2dc3d9afce56e0cb1ff3599c9a79b2dad2d910` |
| structfs-serde-store-0.4.0.crate | `aa14a1c05bdc5e51e9c9abfdce33bbb16f6a989982a015c1001f8876915fa4f9` |
| structfs-service-0.4.0.crate | `8782edf0b33381861033135df571e5756820fa23e8a06014d0c303b8d2255887` |
| structfs-state-0.4.0.crate | `a4c0e988242c61b67eed6e614fe2d431988d89391f2a125e775273292beb0cfe` |
| structfs-sys-0.4.0.crate | `f6ab3cc8f75d9a2a25c9e8c2edc905e5e8aefb18f4cea79dd5e3dd6cb8023c3b` |
