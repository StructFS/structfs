# Local release validation — September 12, 2026

Candidate: StructFS / Featherweight 0.2.0 and Isotope specification 2026-09-12.
These results describe the local release-preparation changes based on repository
commit `027fe72`; no crates, Git tags or websites were published by this work.
Namecode remains at its independently published 0.1.1 version.

## Environment and commands

Host: aarch64-apple-darwin, Rust 1.96.0, Node 25.9.0. Python 3.12 drives the Cargo
archive gate. The workspace HTTP tests required normal macOS SystemConfiguration
access: the sandbox run failed seven HTTP initialization tests; the unrestricted
run passed. Those initial failures were not assertions about HTTP behavior.

```sh
scripts/check-release.sh
python3 scripts/release.py --dry-run --skip-gates
# In featherweight/host/browser:
npm run check
npm test
# After copying isotope/spec/*.md to isotope/site/src/spec/:
pnpm --dir isotope/site exec eleventy
FW_CAPACITY=10000 cargo test -p featherweight-runtime --test capacity --locked --offline -- --nocapture
```

Registry preflight uses the Cargo sparse index. It confirmed that the nineteen
0.2.0 candidates are unpublished and Namecode 0.1.1 is published. An initial
crates.io API query returned 403; this was not interpreted as a missing crate.
Repeat preflight immediately before publication.

## Acceptance evidence

The final complete gate passed **1,331 workspace tests** (25 ignored) and
**664 archive test executions** (15 ignored), with zero failures. Counts include
Rust documentation tests and the separate portable-profile invocation.


- Eight release-driver regressions: publication policy, dependency ordering and
  cycles, filters, private dependencies, registry failures, omitted dependencies,
  dry-run nonpublication and index propagation.
- All-feature workspace tests and strict Clippy; warning-free workspace docs.
- Every facade feature checked independently, including no defaults.
- Actual Cargo archives extracted into a separate workspace; all four independent
  consumers tested there. Guest codec/state/profile modes and portable profile
  builds checked for wasm32-unknown-unknown. Every archive's documentation checked
  with warnings denied. The packaged StructFS quickstart executed successfully.
- Browser-host TypeScript checking and all 21 Node tests passed.
- Isotope site built 19 pages, including the capability-profile chapter and the
  specification candidate link. The site build now requires its frozen lockfile.
- Formatting, shell syntax and diff whitespace checks passed.

Ignored Rust cases are the existing documentation examples and runtime fixture
regeneration/scheduling diagnostics; they are not counted as executed acceptance.
The archive gate can emit Cargo's unused-patch notices for portable feature builds;
these are distinct from rustdoc warnings, which are denied.

## Capacity evidence and the regression it found

The harness creates 10,000 fresh Wasm sessions sharing one prepared artifact,
parks each in a provider read, releases the reads and joins teardown, twice.
It uses four Tokio workers and two blocking workers. This is a debug-profile
correctness/capacity workload, not HTTP throughput or a statistical latency study.

The original large-session invocation exhausted the default 16,384 routed-call
budget after the shared service layer began accounting for nested provider calls.
The harness now explicitly configures 40,000 call slots; it observes 20,000
simultaneous calls. Runtime defaults are unchanged. Early request completion now
reports its error immediately instead of waiting for the provider-count timeout.

That exposed a separate quadratic scan: every supervisor owner allocation swept
all existing owners. With the explicit call budget but before fixing the scan,
the two rounds completed in 8,141 ms and 8,153 ms. After moving reclamation to
capacity pressure, this host measured:

| Measurement | Round 1 | Round 2 |
| --- | ---: | ---: |
| Time through all sessions parked | 300 ms | 277 ms |
| Time through complete teardown | 547 ms | 534 ms |
| Parked process RSS | 1,265,184 KiB | 1,278,656 KiB |
| Process RSS after release | 307,280 KiB | 318,720 KiB |
| Parked process threads | 26 | 26 |
| Peak charged calls | 20,000 | 20,000 |
| Retained block registrations after teardown | 0 | 0 |
| Charged calls / logical bytes after teardown | 0 / 0 | 0 / 0 |

Every assembly reported complete shutdown. Two new service tests prove that
reclamation preserves unfinished cleanup and unacknowledged failures while
admitting replacements for quiescent owners. Quiescent owner records can remain
retained until capacity pressure, bounded by the configured supervisor capacity;
this is the intentional memory/time tradeoff. The runtime's owner cap is 65,536.

RSS includes the whole process and runtime bookkeeping, not only guest memory.
The observed improvement is evidence for this workload on this host, not a
cross-platform performance guarantee or production gateway benchmark.

## Before calling the release externally certified

The Linux/macOS × Rust 1.96/stable CI matrix, browser job and specification-site
job are configured, but hosted execution and branch-protection settings were not
verified from this local workspace. Require their successful results on the final
release commit. Run the actual downstream application's migration suite: the
repository's four independent consumers do not substitute for Ox/Horns remount,
ledger, UI, approval or production adapter tests.

Windows, actual browser engines/worker residency, production socket adapters,
crash-safe durable stores and OS process isolation are not certified by this
candidate. After deliberate publication, repeat smoke tests with registry-only
exact dependencies, inspect docs.rs, and publish the matching reviewed tags/site.
See [the release procedure](releasing.md) and [migration guide](migration-0.2.md).
