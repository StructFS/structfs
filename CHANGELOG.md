# Changelog

## 0.4.0

- Recoverable prepared execution for synchronous and asynchronous host stores,
  with explicit execution owners, joined state recovery, supervisor retention,
  engine-wide epoch cadence, and configurable growth-denial behavior.
- Shared reader/writer capabilities, allocation-free normalized suffix matching,
  hygienic path macros through renamed dependencies and facades.
- Flat-entry MemoryStore snapshots preserving Null and empty containers; optional
  roots distinguish imported Null from an empty store. Ordinary Null writes delete.
- Conventional state assignment translates Null to the internal Delete operation.
- Parsed-only implicit typed reads and canonical explicit server response presence.
- Legacy driver bridges and public unowned core-Wasm run entry points removed.

See [migration](docs/migration-0.4.md), [design decisions](docs/design/2026-09-14-coherent-contracts.md),
and the generated [registry status](docs/release-status.md) for publication state.
Historical validation records do not certify this revision.

## 0.3.0 — published

### Added

- Detached typed reads/writes and detached ReadOnly, Rooted, Masked and Cascade
  composition. `DetachedShared` retains fallback ownership without eager reads.
- Opt-in component-array Serde adapters for Path and Option<Path>, and explicit
  minimum-middle suffix patterns with documented Serde representations.
- Independent browser/native consumer and an executable HTTP disconnect example
  covering late allocation replies, aliased handles, joined cleanup, retained
  capacity and nonzero guest exit; exercised against package archives.

### Fixed

- Path macro expressions require PathComponent; the public hidden constructor
  validates in release builds as well as debug builds.
- Assembly standard fields reject wrong types, unknown fields and unknown block
  references. Only `x-` fields are ignored extensions; config payloads stay open.
- Serde custom diagnostics retain bounded text and nested field/index locations
  alongside the structured error category.
- Workspace dependencies no longer inject a native Tokio executor into portable
  code. HTTP's blocking feature and handles' SyncBridge feature are explicit.

### Migration

`PathPattern` has a new public variant. HTTP without default features now exposes
portable types without native stores. See [0.3 migration](docs/migration-0.3.md)
and [platform support](docs/platforms.md). Registry availability was verified on
2026-09-14; the previous unreleased wording was stale.

## 0.2.0 — published

This coordinated StructFS and Featherweight candidate implements the Isotope
2026-09-12 specification snapshot, Value v1, and optional capability profiles v1.
The StructFS 0.2.0 entry was verified as published and not yanked in Cargo’s
sparse registry index on 2026-09-13.
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
