# Changelog

## 0.2.0 — candidate, not published

This coordinated StructFS and Featherweight candidate implements the Isotope
2026-09-12 specification snapshot, Value v1, and optional capability profiles v1.
Registry verification must confirm that 0.2.0 is available before publication.
Namecode remains independently versioned at 0.1.1.

### Added

- Exact unsigned Values and bounded, checked Serde conversion; canonical tagged
  JSON and native JSON/CBOR/FlexBuffers fidelity contracts.
- Shared async service routing, scoped clients, owned registrations and supervised
  provider cleanup in `structfs-service`.
- Revisioned state, atomic batches, snapshots and bounded observation in
  `structfs-state`.
- Optional discovery, interactive-session and operation contracts in
  `structfs-profiles`; corresponding guest SDK features.
- Prepared artifacts, reusable execution drivers, fresh and persistent instances,
  bounded consuming streams, admission policies and inspectable runtime accounting.
- Independent packaged embedding, reactive-screen, streaming-gateway and
  conversation-service consumers, including actual Wasm guest calls.

### Fixed

- Cleanup supervisor admission no longer scans every live owner for each new
  scope. Reclamation runs at capacity and retains unfinished or failed cleanup.
- The large-session harness reserves nested routed-call capacity explicitly,
  reports early request failures, and verifies complete shutdown and zero charges.
- Release tooling handles publication settings, exact registry versions and
  macOS's default Bash; documentation gates cover every archive.

### Changed / migration required

- `Value::Unsigned` extends the public enum; exhaustive matches must handle it.
- `value_to_json` returns `Result`; plain JSON rejects bytes and non-finite floats.
  Typed conversions reject implicit numeric coercions and ambiguous null options.
- Shutdown returns ownership/cleanup reports. Incomplete cleanup retains charges;
  hosts must retain and join the cleanup owner.
- Timer and signal delivery report admission failures; raw unowned enqueue is
  private. Malformed non-array assembly wiring is rejected.
- Explicit present Null is distinct from an absent server response. Use
  `ok_absent()` when a server intends absence.
- StructFS and Featherweight declare Rust 1.96 as their supported minimum.

See [migration](docs/migration-0.2.md), [release procedure](docs/releasing.md), and
[the specification compatibility matrix](isotope/RELEASE.md) for details and limits.
