# Featherweight embedding release gate — September 11, 2026

The next release can support external runtime adapters without private access.
The independent `tests/embedding` crate is the acceptance fixture: it registers
prepared non-Wasm code, serves a persistent instance, consumes a binary stream,
cancels a request's provider wait, rejects overload, serves a subsequent request,
and joins teardown. It uses only public APIs.

## Implemented release requirements

- Prepared artifact registration and a common `DriverContext` execution entry
  point for built-in and external drivers. Existing loader adapters retain the
  synchronous bridge. Optional host-only resumable/checkpoint control extensions
  do not promise universal snapshot support.
- Separate reusable artifacts, persistent instances and request owners.
  Cooperative request cancellation, instance shutdown reports and panic cleanup
  are covered by tests. Unjoined native work retains its registration.
- Bounded consuming duplex streams with binary payloads, atomic chunks,
  backpressure, readiness, half-close and full release. Event/timer capacity is
  reserved before delivery. Response payloads remain charged until consumed.
- Stable live admission policies with revisioned snapshots; Wasmtime fuel and
  current/peak linear-memory measurements; bounded adapter-specific counters
  with explicit units. Measurements remain inspectable after termination.
- Read-only `iso/capabilities` and `iso/execution/budget`, request cancellation
  queries, and spec 13's separation of runtime-owned services from explicit
  grants. Existing mailbox paths are preserved.
- Actual Cargo archive verification, independent-consumer tests, packaged replay
  fixtures, both Wasm guest SDK feature modes, and strict documentation checks.
  The gate runs in CI and in release preflight. Workspace packaging is batched
  so unpublished local versions resolve together before any publication.

## Run the gates

```
cargo fetch --locked
rustup target add wasm32-unknown-unknown
scripts/check-featherweight-release.sh
```

The script selects a Python interpreter with `tomllib` and safe tar extraction
(Python 3.12 recommended). Its Cargo operations are offline after dependency
fetching. It never publishes or tags. Extracted packages are patched together in
an isolated workspace; the consumer manifest has no source-checkout paths.
Every publishable archive is also checked with all targets enabled. This avoids
Cargo 1.96's temporary-registry hash failure in built-in batch verification while
still compiling actual package contents. Timestamp normalization is undone before building so cached test binaries cannot
retain paths into an earlier, removed extraction directory.

The complete workspace tests passed: 1,275 passed, zero failed, 23 ignored.
The archive gate passed 162 tests including documentation tests, with zero failures
and three ignored diagnostics. Workspace Clippy passed with warnings denied.
The full test suite required execution outside the macOS sandbox because existing
HTTP client tests use SystemConfiguration; the initial sandbox run failed there.

The 10,000-session capacity harness also passed two rounds: 494 ms and 479 ms
through teardown, with zero retained registrations, 26 process threads while
parked, approximately 1.02 GiB parked RSS and 264 MiB after release. This is the
lightweight Wasm/provider harness, not HTTP throughput or container capacity.

## API migration notes

- `AssemblyInstance::shutdown` returns a report. Hosts must not release instance
  reservations while `remaining` is nonempty.
- `deliver_signal` and `deliver_timer` return `Result` on admission failure;
  `AssemblyInstance::signal` returns false when delivery is rejected.
- Raw unowned `BlockCell::enqueue` is no longer public. Assembly calls own their
  correlation and resource cleanup, including on timeout or dropped futures.
- Malformed non-array assembly `wiring` now fails instead of being ignored.
- Binary streams consume data. Existing append-only `ByteStream` keeps its prior
  semantics and is not a substitute for a bounded connection buffer.

These API changes need the appropriate release version under the project's
versioning policy. This work does not publish crates or choose that version.

## Deliberate boundaries

Adapters and provider code are trusted host code; they must enforce or reject
requested policies, own and join any tasks, and report supported measurements.
A request timeout cannot undo an effect or forcibly interrupt unrelated work in
a shared interpreter. Fuel units are not CPU time, RSS, or emulated instructions.
Admission updates do not change a running guest's fuel or memory ceiling.

No container port, network listener manager, persistent filesystem, universal
snapshot ABI or debugger implementation is included. Spec 13 provides extension
points and contracts for that work. Existing ambient `iso` convenience services
remain a compatibility profile; newly granted network and configuration services
belong outside the reserved prefix.
