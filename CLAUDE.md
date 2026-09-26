# Claude Code Instructions for StructFS

## Project Overview

StructFS is a Rust workspace that provides a uniform interface for accessing data through read/write operations on paths. The core philosophy is "everything is a store" - all data access, including mount management, HTTP requests, and configuration, happens through the same read/write interface.

## Workspace Structure

Every workspace member, with a one-line summary:

```
packages/
├── ll-store/         # Low-level StructFS store traits - pure bytes, no semantics
├── path-validation/  # Shared path component validation rules for StructFS
├── path-macro/       # Compile-time validated path! macro for StructFS
├── core-store/       # Core StructFS store traits - Record, Value, Path, Format
├── serde-store/      # Serde integration for typed StructFS access
├── json_store/       # JSON-based store implementations for StructFS
├── handles/          # Handle-store and streaming primitives: outstanding/{id}
│                     # scaffolding, bounded duplex byte streams, cancellable
│                     # parked reads
├── service/          # Scoped async service routing, provider adapters, and
│                     # shared call admission
├── state/            # Revisioned tree state, atomic batches, and bounded observation
├── profiles/         # Versioned capability profiles and headless interactive sessions
├── http/             # HTTP client stores for StructFS
├── sys/              # OS primitives exposed through StructFS paths
├── repl/             # Interactive REPL for StructFS (the `structfs` binary)
└── structfs/         # Feature-selectable library facade over the crates above

featherweight/        # Featherweight Isotope runtime CLI (`fw`): run assemblies
│                     # and the demo shell
├── runtime/          # Strawman Isotope runtime: blocks, assemblies, namespaces,
│                     # and the /iso store over StructFS
├── component/        # WIT component-model binding adapter: run wasm components
│                     # as Isotope blocks
├── guest/            # Guest SDK and reference block for the core-wasm Block ABI
│                     # binding (isotope/spec/11)
└── wasi/             # WASI-over-Isotope shim core: the syscall surface
                      # implemented over the Block ABI (isotope/spec/10)

namecode/             # Encode Unicode strings as valid programming language
                      # identifiers (independently versioned)
isotope/              # The Isotope virtual-OS specification (not a crate)
```

## Key Concepts

- **Stores**: Implement `Reader` and `Writer` traits for path-based data access
- **Value**: Core data type (Null, Bool, Integer, Unsigned(u64), Float, String,
  Bytes, Array, Map)
- **Record**: Wrapper for raw bytes or parsed Value
- **`path!` macro**: Proc macro validating literals at compile time; runtime
  components must be `PathComponent` values (`try_new` validates; `encode`
  makes any string valid via Namecode). It expands hygienically through
  renamed dependencies and the facade
- **Store conventions**: Reading a prefix returns a map of children; writing
  `Null` deletes the subtree; writing a map replaces the subtree; deep writes
  create intermediates. Certified by `core-store`'s `conformance` module;
  `MemoryStore` is the reference implementation
- **`Reader::read_children`**: Enumerate child names at a prefix (defaulted
  from the map convention; stores can override); `read_children_page` /
  `ChildNames` / `ChildPage` are the paged names-only projections
- **Combinators**: `ReadOnly`, `Cascade` (layering), `Shared` (Arc<Mutex>
  handle), `Rooted` (subtree confinement), `Masked` (component-wise read
  redaction) in `core-store`
- **Append logs**: `LogStore` + `AppendBacking`/`JsonlFileBacking` in
  `json_store` — write to `append`, page with the `entries/from/{n}`
  cursor tail; `Durability::Synced` plus `recover` give explicit persistence
  acknowledgement. The background HTTP broker (`BackgroundHttpBrokerStore`)
  is a `HandleStore` instantiation.
  The REPL mounts them (`{"type": "log", "path": ...}`), and mounts a
  whole `fw --record` directory read-only (`{"type": "recording"}`):
  session timeline at `session`, block transcripts at their keys
- **PathPattern**: Component-wise Exact/Prefix/PrefixSuffix matching
- **Typed access**: `read_typed`/`write_typed` on any store (codec-free, in
  `serde-store`); `Path`/`Value`/`Record`/`Format` all implement serde
- **Typed errors**: Prefer `Error::NotFound`/`PermissionDenied`/`Conflict`/
  `InvalidArgument`/`Overloaded`/`DeadlineExceeded`/`ResourceLimit` over
  stringly `Error::Store`
- **MountStore**: Routes operations to different stores based on path prefixes
- **OverlayStore**: Mounts stores at paths, creating a unified tree
- **Broker pattern**: the sync HTTP broker (`HttpBrokerStore`) queues requests
  on write and executes on read; the background broker
  (`BackgroundHttpBrokerStore`) starts them on write and parks on
  `response/wait`
- **Handle protocol** (`structfs-handles`): generic `outstanding/{id}`
  scaffolding (`HandleStore`), bounded consuming byte streams
  (`DuplexStream`), lost-wakeup-proof parking (`Gate`), and
  cancellation that fails reads but not writes (`CancelToken`); certified by
  its own conformance module. `service::SupervisedProtocol` joins handle
  cleanup under an owner
- **Isotope runtime** (`featherweight/`): blocks serve stores via the
  server protocol (requests read from `iso/server/requests`), assemblies
  wire per-block capability namespaces, `fw shell` runs the demo. The
  Block ABI is a semantic contract (spec 10); the runtime core speaks
  only the core-wasm binding (spec 11, module `structfs`, no tooling —
  the SDK binding). Other bindings are adapters via
  `RuntimeConfig::register_loader`: `featherweight-component` runs WIT
  component artifacts (the core has no WIT knowledge); WASI is a shim
  above the ABI (`featherweight-wasi`), never a runtime dep
- **Async**: `AsyncReader`/`AsyncWriter` (borrowed futures) plus
  `DetachedReader`/`DetachedWriter` (futures that don't borrow the store,
  for concurrent in-flight operations) and the erased `SharedReader`/
  `SharedWriter` client capabilities
- **Persistence**: `Backing` trait + `BackedStore`/`JsonFileBacking` in
  `json_store`
- **Docs protocol**: Stores can provide documentation at a `docs` path

## Development Commands

These are the commands `scripts/quality_gates.sh` runs (the script adds
`--check` to fmt and `--quiet` to clippy):

```bash
# Run all quality checks: fmt --check, clippy, tests, and a 90% region-coverage
# gate (coverage is skipped when cargo-llvm-cov is not installed)
./scripts/quality_gates.sh

# Individual commands
cargo fmt --all
cargo clippy --workspace --all-features --all-targets --locked -- -D warnings
cargo test --workspace --all-features --locked

# Run the REPL
cargo run -p structfs-repl

# Test coverage (requires cargo-llvm-cov)
./scripts/coverage.sh              # Summary with top 10 gaps
./scripts/coverage.sh gaps         # All files below 95%
./scripts/coverage.sh file src/foo.rs  # Uncovered lines in a file
./scripts/coverage.sh html         # HTML report in browser
```

## Release fixtures

`tests/embedding`, `tests/portable` and `tests/applications/*` are standalone
Cargo projects (not workspace members) that consume the crates as an
independent user would. `scripts/check-release.sh` and
`scripts/check-featherweight-release.py` build them against the workspace via
path dependencies and against extracted package archives; they are not run by
`cargo test --workspace`. The wasm-only `featherweight-guest` is natively
tested by `scripts/check-featherweight-release.py`. See `docs/releasing.md`.

## Code Style

- Keep solutions simple and focused - avoid over-engineering
- Prefer editing existing files over creating new ones
- Error types: `http`, `repl` and `featherweight-runtime` use `thiserror`;
  `core-store` and the other substrate crates hand-write `Display`/`Error`
  impls. Either is fine; match the crate you are in
- Use `serde` for serialization; JSON, CBOR, and FlexBuffers are
  equivalent-tier transports (`MultiCodec::standard()` in `serde-store`),
  with JSON the human-facing default

## Commit Messages

- Use conventional commits (`feat:`, `fix:`, `refactor:`, etc.)
- Describe what changed and why — never use subjective quality labels like "S-tier", "world-class", "best-in-class", "premium", etc.
- Keep the subject line under 72 characters; use the body for detail

## Architecture Decisions

1. **Three-layer architecture**:
   - `ll-store`: Pure bytes, no semantics
   - `core-store`: Record/Value abstraction, path routing
   - `serde-store`: Serde integration for typed access

2. **Provider interfaces**: Stores support synchronous, borrowed async, detached,
   and shared-client access with explicit concurrency contracts. The sync HTTP broker uses a deferred execution pattern (write queues, read executes).

3. **Path-based routing**: Paths are the universal addressing mechanism. The `Path` type normalizes trailing slashes and validates components.

4. **Mutable provider Reader/Writer traits**: Both `read()` and `write()` take `&mut self`.
   This is intentional—some stores (HTTP broker, filesystem) have state that
   changes on read. Using `&mut self` uniformly avoids the complexity of split
   traits or interior mutability. For concurrent access, wrap stores in
   `Arc<Mutex<_>>` explicitly.

5. **Default context mounts** (`DEFAULT_MOUNTS` in `repl/src/mounts.rs`): The
   REPL provides built-in stores at `/ctx/*`:
   - `/ctx/repl` - REPL documentation
   - `/ctx/http` - Background HTTP broker (`BackgroundHttpBrokerStore`)
   - `/ctx/http_sync` - Sync HTTP broker (blocks until complete)
   - `/ctx/sys` - OS primitives (env, time, proc, fs, random)
   - `/ctx/registers` - Registers (session-local named values)
   - `/ctx/help` - Documentation system

## Common Patterns

### Adding a new store type

1. Implement `Reader` and `Writer` traits from `structfs_core_store`
2. To make it mountable in the REPL, add a variant to the REPL's
   `MountConfig` in `repl/src/mounts.rs` (serde-tagged by `type`; add it
   to `MountConfig::TYPES` too) and build it in `CoreReplStoreFactory::create`
   in the same file

core-store's `MountStore<F: StoreFactory>` owns only the mechanism: the
factory names its `Config` type, decodes it from the `Value` written to
`ctx/mounts/<name>` (`config_from_value`), encodes it for the listing
(`config_to_value`), and builds the store (`create`). Other embedders
define their own factory and config type.

### The HTTP broker pattern

The sync broker at `/ctx/http_sync` queues on write and executes on read:

```bash
# Write queues the request, returns handle path
write /ctx/http_sync {"method": "GET", "path": "https://example.com"}
# Returns: /ctx/http_sync/outstanding/0

# Read from handle executes the request (blocks), then caches the response
read /ctx/http_sync/outstanding/0
# Returns: HttpResponse with status, headers, body
```

The background broker at `/ctx/http` starts the request on write. Reading the
handle returns its `RequestStatus` without waiting; read `response` (absent
while pending) or park on `response/wait`:

```bash
write /ctx/http {"method": "GET", "path": "https://example.com"}
# Returns: /ctx/http/outstanding/0
read /ctx/http/outstanding/0                 # RequestStatus
read /ctx/http/outstanding/0/response/wait   # HttpResponse, once ready
```

### The sys store

OS primitives exposed through paths:

```bash
read /ctx/sys/env/HOME           # Environment variables
read /ctx/sys/time/now           # Current time (ISO 8601)
read /ctx/sys/random/uuid        # Random UUID v4
read /ctx/sys/proc/self/pid      # Process ID
write /ctx/sys/fs/open {"path": "/tmp/file", "mode": "write"}  # File handles
```

### REPL Registers

Registers store command output for later use:

```bash
@handle write /ctx/sys/fs/open {"path": "/tmp/test", "mode": "write", "encoding": "utf8"}
write *@handle "Hello"           # Dereference to use as path
read @handle                     # Read register contents
write *@handle/close null        # Close the handle
```

(fs handles default to base64; `"encoding": "utf8"` makes the plain string
write work. `commands::tests::claude_md_register_example_writes_a_file`
runs this sequence.)

## Testing

Unit tests live alongside code in `#[cfg(test)]` modules. Current coverage: ~92% region coverage (the gate measures regions).

### Testing Infrastructure

The codebase uses dependency injection to enable testing without external
dependencies. The two test doubles below are `#[cfg(test)]`, so they exist
only inside their own crate's unit tests; other crates and the release
fixtures cannot import them.

**TestHost** (`packages/repl/src/io/test_host.rs`, crate-internal): in-memory
`IoHost` for driving `ReplCore::run` without a terminal.
```rust
let mut host = TestHost::new();
host.queue_inputs(["read /ctx/sys/time/now", "exit"]);
ReplCore::new().run(&mut host)?;
let text = host.output_text();
let prompt = host.last_prompt();
```

**BlockingHttpExecutor / MockExecutor** (`packages/http/src/executor.rs`):
`BlockingHttpExecutor` is the public seam; `BlockingReqwestExecutor` is the
real one. `executor::mock::MockExecutor` is crate-internal and returns canned
responses without network.
```rust
let executor = MockExecutor::new()
    .with_response("/api/data", MockExecutor::success_response(json!({"key": "value"})));
let store = HttpBrokerStore::with_executor(executor);
```
Outside the http crate, implement `BlockingHttpExecutor` yourself and pass it
to `HttpBrokerStore::with_executor`, `BackgroundHttpBrokerStore::with_executor`
or `HttpClientStore::with_executor`.

**StoreFactory injection**: `StoreContext` is generic over a `StoreFactory`
(`StoreContext::with_factory`), for testing with mock stores.

### Untestable Code

Some files remain at 0% coverage by design:
- `repl/src/host/terminal.rs` - Real terminal I/O (reedline integration)
- `repl/src/lib.rs`, `repl/src/main.rs` - Entry points

## Files to Know

- `packages/core-store/src/path.rs` - Path parsing and validation
- `packages/core-store/src/mount_store.rs` - `MountStore<F: StoreFactory>` and the `StoreFactory` trait
- `packages/core-store/src/overlay_store.rs` - OverlayStore for composing stores
- `packages/http/src/client_store.rs` - `HttpClientStore` (read GET, write POST)
- `packages/http/src/sync_broker.rs`, `background_broker.rs`, `broker_common.rs` - the two HTTP brokers and their shared paths/docs
- `packages/http/src/executor.rs` - `BlockingHttpExecutor`, `BlockingReqwestExecutor`, and the test-only `mock::MockExecutor`
- `packages/sys/src/lib.rs` - SysStore with all sub-stores
- `packages/repl/src/mounts.rs` - REPL `MountConfig`, `CoreReplStoreFactory`, and `DEFAULT_MOUNTS`
- `packages/repl/src/store_context.rs` - `StoreContext`: mount/unmount and help indexing
- `packages/repl/src/help_store.rs` - Help system
- `packages/repl/src/commands.rs` - Command parsing, register handling, dereference syntax
- `packages/repl/src/io/test_host.rs` - TestHost for testing REPL without terminal
- `docs/releasing.md` - Release procedure; `docs/release-status.md` - generated registry state
- `docs/history/` - Dated validation/audit records and the completed `plans/` (not living docs)

## Contract boundaries (0.4, carried into 0.5)

Isotope exposed stores follow the Null-as-deletion convention; internal data
structures, snapshot construction and state batch operations may preserve Null.
Use `MemoryStore::from_entries` for explicit snapshot import. `SharedReader` and
`SharedWriter` are the erased shared client capabilities (0.5 renames their
methods to `read_shared`/`write_shared`). All implicit typed reads
(`read_typed`) are parsed-only; raw records need `read_as` with a codec. For
core-Wasm embedding, `CoreWasmEngine::prepare` the code, then
`start_sync`/`start_async` with an explicit `CleanupSupervisor`, and join the
`ExecutionOwner` to recover host state. See docs/migration-0.4.md,
docs/migration-0.5.md (the 0.5 breaking cleanup, including the
`close`/`join` stop verbs and the facade feature renames) and
docs/history/plans/02-coherent-contracts.md.
