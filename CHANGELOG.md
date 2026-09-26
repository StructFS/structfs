# Changelog

## Unreleased (0.5.0)

0.5 is a breaking release guided by the
[2026-09-17 code audit](docs/code-audit-2026-09-17.md). See
[migration](docs/migration-0.5.md) for rename tables.

### Added

- core-store: `Error::InvalidArgument` for caller mistakes, distinct from
  `Conflict` (state conflicts) and `ResourceLimit` (capacity).
- core-store: `OverlayStore` and `MountStore` forward `read_children` and
  `read_children_page`; conformance checks paging and raw-record writes.
- core-store: `ChildPage::new`, `TypeInfo::with_schema`, `CodecOperation::as_str`,
  `FromStr` for `CodecErrorKind`/`CodecOperation`, `MountStore::factory()`.
- namecode: `encode_forced`, `is_encoded`, `DecodeError::NonCanonical`.
- serde-store: `Limits::with_max_*` setters, `CodecProfile::has_canonical_form`,
  `write_typed_async`, `Debug` for `MultiCodec`.
- json_store: `with_limits` on `JsonFileBacking`/`JsonlFileBacking`;
  `last_failure_ambiguous()` on both backing traits.
- handles/service/state/profiles: `join(timeout)` on every owned resource with
  cleanup to wait for; `OwnerHandle::register_with_cancellation`; `with_*`
  setters on `OwnerLimits`, `CallLimits`, `StateLimits` (incl. `max_faults`),
  `ReadLimits`; `new` constructors for the request and input types callers
  build (`Mount`, `Request`, `Node`, `Token`, `Declaration`, `InputEnvelope`,
  `Approval`, `CommitAck`, `ProcessRequest`, `HandleCx`, …). Response and
  report types (`CloseReport`, `TailRead`, `Descriptor`, `SnapshotPage`,
  `SessionStatus`, …) are read-only.
- http: `HttpRequest::url()`, `Method::{CONNECT, TRACE}`,
  `Error::{ClientBuild, Status}`, `SseFramer::{reconnection_time, last_event_id}`,
  `HttpResponse` builders.
- sys: `FsStore::rooted`/`SysStore::rooted` directory confinement (symlinks
  resolved), `fs/readdir`, real `fs/stat`, read caps (`DEFAULT_MAX_READ_LEN`),
  handle cap (`DEFAULT_MAX_HANDLES`), `MAX_SLEEP`, `read random/bytes/{n}`.
- repl: `mounts` and `ls` commands; `{"type":"local"}` (persisted JSON file) and
  `{"type":"http"}` (`HttpClientStore`) mounts; nested register writes
  (`write @reg/child v`); `EditMode` with flag > `STRUCTFS_EDIT_MODE` >
  vi-family `EDITOR`/`VISUAL` > inputrc > emacs precedence.
- structfs facade: `handles` feature; portable `net` feature (no reqwest).
- featherweight: frozen `RuntimeConfig` (`RuntimeConfig::new(handle)`,
  `with_*`, `register_*`, then `Runtime::new(config)`), including
  `with_core_engine` to share one engine across runtimes; read-only
  `BlockView`; `protocol::ErrorKind` as the single error taxonomy
  (`from_status`, `table_json`, committed `error-kinds.json` for other hosts);
  `RuntimeError::{Admission, Policy, EngineConfig, AlreadyJoined,
  ExecutionLost, HostPanic}`; `CoreWasmEngine::available_sessions()`;
  `ComponentEngine`/`PreparedComponent`; guest `sdk::status` constants;
  `RuntimeError::task_failed` (a panicked runtime task is `HostPanic`, a
  cancelled one `ExecutionLost`).
- state: `StateLimits::max_handles` / `with_max_handles` (global live-handle
  ceiling, default 256). service: `BudgetAdmission::new`. wasi:
  `OpenFlags::with_{read,write,create,truncate,append}`. core-store:
  `From<Shared<T>> for SyncToAsync<T>`. serde-store re-exports `path!`.

### Changed

- **Facade layout.** Every item has one path. Features and modules renamed so
  `use structfs::*` no longer shadows the `serde`/`http` crates: `serde`→`typed`,
  `json`→`persist`, `http`→`net-blocking` (new `net` is schema only),
  `http-streaming`→`net-streaming`, `sys`→`os`. Byte-layer types moved to
  `structfs::ll`; `PathPattern`/`matches_prefix_suffix` to `structfs::pattern`.
  `service` implies `handles`.
- **Mount configuration.** `MountConfig` moved from core-store to
  `structfs_repl::MountConfig`. `StoreFactory` gains `type Config` plus
  `config_from_value`/`config_to_value`; `MountStore<F>` stores `F::Config`.
  Mount names are normalized paths; a duplicate name is `Conflict`; `unmount` of
  a missing name is `NotFound`; `ctx/mounts` lists names from `read_children`.
  `MountStore::list_mounts` returns `Vec<(String, Option<F::Config>)>`.
- **Persistence format.** `JsonFileBacking`/`JsonlFileBacking` write the tagged
  StructFS Value JSON v1 envelope, so bytes, non-finite floats and `u64` survive.
  0.4 plain-JSON files still load; 0.4 silently misreads files written by 0.5
  (it parses the `["structfs-value",1,…]` envelope as a plain 3-element
  array). Loads and saves are bounded by `Limits`; tagged values nest at most
  84 levels. Only failures that may have touched disk fence a store until
  `recover`. `BackedStore::root()` returns `Option<&Value>`. `LogStore`
  rejects appending Null with `InvalidArgument`.
- **namecode.** `encode("")` is `"_N_"`; every `_N_`-prefixed input is encoded,
  so `encode` is no longer idempotent; `decode` rejects uppercase digits and
  non-canonical input. MSRV 1.96.
- **PathComponent.** `encode` passes numeric strings through (`"42"` stays
  `"42"`; 0.4 gave `_N_42`) and never yields an invalid component
  (`encode("_")` is `_N__`, `encode("")` is `_N_`). `decode` still accepts
  `_N_42`, so stored names can be migrated by decoding and re-encoding.
- **serde-store.** `Profile`→`CodecProfile`; `ValueCodec::canonical()` returns
  `Result`; `ValueCodec::require_canonical` is private (set it with
  `canonical()`, read it with `requires_canonical()`); codec-taking typed
  methods take `Arc<dyn Codec>`; `Limits`, `CodecProfile`, `ValueCodec` are
  `#[non_exhaustive]`.
- **core-store.** `LazyRecord::into_record` returns `Result`. `Format` is
  `#[non_exhaustive]` (build it with the constants, `Format::new` or
  `Format::from_static`, not a `Format(cow)` literal); `PathPattern`,
  `RedirectMode` and `CodecOperation` need wildcard match arms.
- **Shared client traits.** `SharedReader::read`→`read_shared`,
  `SharedWriter::write`→`write_shared`.
- **Stop verbs.** `close()` requests cleanup without blocking; `join(timeout)`
  waits and reports. Renamed on `Owner` (`cancel`→`close`, `close(t)`→`join(t)`,
  `join()`→`join_indefinitely()`), `OwnerHandle`, `Registration`,
  `OwnedResource`, `OwnedTail`, `CleanupSupervisor`, `BlockingStore`,
  `DuplexStream` (`release`→`close`), `StateHandle`, `Session`,
  `OperationHandle`. Exceptions: `StateHandle::close` is `async` and returns
  `Result<(), ClientError>`, with no `join`; `BlockingStore::join(t)` closes
  first and returns `bool`; `DuplexStream` has only `close`. Featherweight
  `ExecutionOwner::cancel`→`close` (`join`/`wait` already wait); guest SDK
  `state::Handle::release`→`close`. `ExecutionScope::cancel` and
  `AssemblyRequest::cancel` keep their name: they cancel in-flight calls and
  own no resources to clean up.
- **Constructors** return `Self` (within `Result` where fallible) instead of
  `Arc<Self>`: `Router::new`, `CallBudget::new`, `Profiled::new`,
  `OperationHandle::start`, `HeadlessHost::open`;
  `Router::shared`/`CallBudget::shared` wrap in `Arc`. `DuplexStream::pair`
  returns `Result<(Self, Self), Error>`, and `DuplexStream::store` takes
  `self: &Arc<Self>`. `State::shared` is the only `State` constructor.
- **State provider.** Handle ids are random decimal `u64`; handles and faults
  are scoped to the opening view and owner (clients sharing a view without an
  owner share it); foreign releases are silent no-ops. Rejected commands take
  no handle slot; faults and live handles are each bounded per principal
  (`handles`) and globally (`max_faults`, `max_handles`), so one view or owner
  holding its whole budget no longer makes every other view `Overloaded`.
  Batches cost three round trips; `changes` checks the epoch locally.
- **Wire changes.** `StreamReadiness` key `released`→`closed`. A released
  supervised handle reads `Ok(None)`. Error reclassifications to
  `InvalidArgument` across core-store, handles, service, state, profiles, http,
  sys; a `HandleStore` write to a non-handle path or below an unknown handle
  is `NotFound`.
- **http.** Renames: `AsyncHttpBrokerStore`→`BackgroundHttpBrokerStore`,
  `HttpExecutor`→`BlockingHttpExecutor` (returns `crate::Error`),
  `ReqwestExecutor`→`BlockingReqwestExecutor`; `From<http::Method>` is
  `TryFrom`. `Error::Http` exists only with the `blocking` or `streaming`
  feature. Errors are typed: transport timeout → `DeadlineExceeded`; a
  connect failure whose source is an io `ConnectionRefused`/`ConnectionReset`
  → `Overloaded` (other connect failures such as DNS or TLS stay `Store`);
  bad URL, method or header → `InvalidArgument`. Response statuses become
  errors only in `HttpClientStore`: 401/403 → `PermissionDenied`, 404 →
  `NotFound` (on writes; a 404 read is `Ok(None)`), 408/504 →
  `DeadlineExceeded`, 429/503 → `Overloaded`, other non-2xx → `Store`. The
  brokers return non-2xx responses as ordinary `Ok` responses.
  `HttpRequest` rejects unknown fields. At an `HttpClientStore` root, a map
  with a `method` or `path` key is a request and anything else is POSTed;
  broker root writes must parse as an `HttpRequest`. Unknown handles and meta
  paths read `Ok(None)` in both brokers; a non-Null write to an unknown
  handle is `NotFound`, a Null write is a silent no-op. A broker root write
  that is not an `HttpRequest` is `InvalidArgument` (was a JSON `Codec`
  decode error).
  `StreamingResponse` no longer exposes reqwest/http types. SSE ids and
  `retry` persist per the WHATWG spec.
- **sys.** `random/u64` and `time/monotonic` return `Value::Unsigned`; env
  writes go to an in-process overlay (the process environment is never
  mutated); unknown open modes/encodings are `InvalidArgument`; `create_new`
  accepted (`createnew` alias); handle ids are per store, capped at
  `DEFAULT_MAX_HANDLES` open handles (`ResourceLimit` beyond); writing Null to
  `fs/handles/{id}` closes it; `write fs/stat` returns a `results/{id}` path
  holding the metadata (0.4 returned the request path), as does the new
  `fs/readdir`, with at most `DEFAULT_MAX_RESULTS` answers retained;
  `random/bytes/{n}` is capped at `MAX_RANDOM_BYTES`; root and sub-store
  roots list children.
- **repl.** `asynchttpbroker` mount type is `backgroundhttpbroker` (old tag
  accepted); `run()` became `run(edit_mode: Option<EditMode>) ->
  Result<ExitReason, IoError>`; help index caches per mount; `--vi`/`--emacs`
  beat `STRUCTFS_EDIT_MODE`, `EDITOR`/`VISUAL` and the inputrc.
- **featherweight runtime.** `HostStore` is opaque and async-only; transcript
  and session logging are async end to end. `BlockCell`, `BlockEvent`,
  `ServerRequest`, `GrantStore`, `SessionLog`, `Target`, `WiringTable` are
  crate-private; `AssemblyInstance::cell(name)` returns
  `Option<BlockView>`, and `public_cell()` and `Namespace::cell()` return
  `BlockView`. `Metering` carries only fuel; epoch interruption is per engine
  and the built-in loader shares one engine per runtime (default cap 10,000
  concurrent core-wasm runs). `CoreWasmBlock` is always prepared
  (`from_file` removed; use `engine.prepare(bytes)`), and
  `CoreWasmBlock::manifest()` returns `&[u8]`. `shutdown(timeout)` works
  to one deadline for the whole tree. Engine hooks moved to the
  `#[doc(hidden)]` `adapter` module, which re-exports `wasmtime`. A failed
  compile/component worker join is `HostPanic` or `ExecutionLost` (was
  `Wasm`); a store without an execution policy is `Policy` (was `Wasm`).
- **Isotope wire.** Spec 11 status -10 is invalid argument (was -9 other).
  Server-protocol errors add `no_route` and
  `invalid_argument` types plus optional `path`, `component`, `position`.
  Transcript and session-log kinds add `invalid_argument`, `invalid_path`,
  `codec`. Every core error variant keeps its kind across a block hop, and the
  browser host records and replays the same labels as the native host.
- **featherweight component/guest/CLI.** The component adapter's `WasmBlock`
  and its sync `run` are replaced by `ComponentEngine`/`PreparedComponent`
  (which exposes only `manifest()`; run components through
  `ComponentLoader`). `ComponentLoader` is no longer a unit struct: use
  `ComponentLoader::new()` or `ComponentLoader::with_engine(engine)`.
  `featherweight-guest` no longer enables `reference-guest` by default. The
  `fw` CLI uses clap; flags are unchanged.
- **Dependencies.** `wasmtime` is a caret requirement for libraries (only `fw`
  pins `=48.0.1`); `rand` 0.9; `serde_yaml` replaced by `serde_norway`;
  unused dependencies removed across the workspace.
- `#[non_exhaustive]` on public enums and configuration structs throughout,
  including state `ClientError`, `Request`, `Projection`; service
  `BudgetAdmission` (build with `BudgetAdmission::new`), `CallContext`;
  featherweight `AssemblyInstance`; wasi `OpenFlags` (build with `default()`
  plus `with_*`); guest `sdk::{ValueError, CodecDiagnostic, HostError}` and
  `state::Error`; repl `DefaultMount`, `DocsManifest`, `RedirectInfo`,
  `HelpStoreState`, `CommandSpec`, `InputLine`, `Output`, `PromptConfig`,
  `CommandResult`, `ContextError`. Deliberately exhaustive: guest `sdk::Ret`
  (a fixed `repr(C)` ABI record) and serde-store `ExplicitOption` (a newtype
  whose tuple constructor is the API).
- core-store: `SyncToAsync` is a thin view over `Shared` (one lock, one poison
  policy).

### Removed

- core-store: `Path::{from_ll_unchecked, to_ll_path, try_from_ll_path,
  from_validated_components}`, `PathComponent::validated_str`,
  `Format::{is_json, is_protobuf, is_value}`, `LLToCore`, `AsyncLLToCore`,
  `AsyncCoreToLL`, `OnlyReadable`, `OnlyWritable`, `SubStoreView`, nine
  `OverlayStore` methods and deprecated shims, `MountInfo`,
  `PathError::InvalidPath`, the duplicate `overlay_store::Store` trait
  (`overlay_store::Store` is now a re-export of the root `Store`). ll-store:
  `ll_path`, `ll_path_from_strs`.
- serde-store: `read_json*`, `write_json*`, `write_as*` (use `write_typed*`),
  `validate_value`, `MultiCodec::with_json`, and the re-exports `PathError`,
  `Store`, `AsyncStore`, `DetachedStore`, `SyncToAsync`, `SyncToAsyncLL`,
  `AsyncLLReader`, `AsyncLLWriter`, `AsyncLLStore`, `AsyncCoreToLL`,
  `AsyncLLToCore` (import the survivors from core-store).
- json_store: `InMemoryStore`, `in_memory`, `value_utils` (use `MemoryStore`).
- handles: `SyncBridge` and the `sync-bridge` feature, `TailLog`, `TailPage`,
  `ByteStream`, `ByteChunk`. service: `RetainedBytes`, `Lease::strong_count`,
  `CancelToken` re-export (use `structfs_handles::CancelToken`), the
  `AsyncReader`/`AsyncWriter`/`DetachedReader`/`DetachedWriter` impls on
  `Client`. profiles: `Token` re-export (use `structfs_state::Token`).
- http: `SerializableReference`, `SerializableTypeInfo` (core `Reference` now
  derives serde with the same wire form).
- repl: `{"type":"structfs"}` mount (never implemented), `ContextError::Http`,
  `read_as_json`, `write_json`. sys: `write random/bytes`.
- featherweight: `Runtime::with_handle`, argument-less `Runtime::new`,
  `Default for Runtime`, the `Runtime::with_*`/`register_*` builders (moved to
  `RuntimeConfig`), `SessionBudget`/`SessionPermit`/`SessionLimits`/
  `SessionBudgetSnapshot`/`SessionUsage`,
  `DriverCapabilities`, `DriverControl`, `AssemblyInstance::{driver_control,
  provider_owner}`, `Runtime::with_provider_limits`,
  `CoreWasmEngine::epoch_interval`, `Metering::disabled`, sync `Reader`/`Writer`
  impls on `HostStore` and `GrantStore`. featherweight-component:
  `WasmBlockState` is private (it was reachable only as unused public API).
- Unused dependencies: core-store `unicode-ident`, service `async-trait`.

### Fixed

- Unsound safe `Path::from_ll_unchecked` removed.
- `HandleStore` no longer leaks a tombstone per abandoned release, and reaps
  owner-cancelled handles; supervised handles no longer linger as zombies after
  an owner closes during open.
- State provider: validation no longer re-encodes the whole tree per mutation
  under its lock; faults cannot exhaust handle slots or be read, released or
  evicted by other views.
- `CancelOnDrop` no longer cancels a call that completed.
- `HttpClientStore` root writes of arbitrary maps POST instead of silently
  sending a GET; a mistyped request never becomes a POST.
- sys: unbounded `len/{n}` allocation, negative sleep/seek wrap-around, and
  `random/bytes` failures on `/+=` encodings.
- repl: `mounts`/`ls` were advertised but unimplemented; prompt mount count;
  inputrc comments no longer switch to vi.
- namecode: non-canonical inputs broke round-trips (debug panic).
- Tagged JSON encoder could write values deeper than its decoder accepts.
- Persisted Null root reopens as an empty store.
- featherweight: a wasm block with an async or service session/transcript
  store no longer panics on `block_on` inside a runtime.
- featherweight: owned execution (`start_sync`/`start_async` and
  `ExecutionOwner::join`) no longer panics. A run whose worker never started
  returns the host with `ExecutionLost`; a host that panicked is returned with
  `host_panicked` set. The host is lost only if the engine panics outside
  every guarded region, which `join` reports as `HostPanic`.
- featherweight: a failed `instantiate` deregisters every block it created
  (nested included), joins started driver tasks under the cleanup supervisor,
  and releases transcript-key claims so retries keep their identities.
- featherweight: released spawn handles shut children down under supervision;
  driver task handles are stored atomically with spawning; `iso/stdio/stdin`
  runs on the blocking pool and honours shutdown; timers end with the block;
  `iso/meta` lists every served path; simulation handles deadlocks in a
  deterministic order.
- featherweight: `serde_yaml` (archived, RUSTSEC-2024-0320) replaced by
  `serde_norway`.
- State provider: one principal holding `limits.handles` live handles no
  longer starves every other view of handles.
- featherweight site: `build.sh` compiles the TypeScript browser host
  (`npm run build`) and serves its `.js` output; it copied `.mjs` files that
  no longer exist.
- repl help: the docs redirect example is `/ctx/help/ctx/sys`; the "Make HTTP
  request" example uses the execute-on-read `/ctx/http_sync` broker.
- tests/embedding exercises owned execution (`start_async` under a
  `CleanupSupervisor`, `ExecutionOwner::close`/`join`) as an outside
  consumer, replacing the deleted copy of the runtime's own suite.

## 0.4.0 — published

Published for all workspace crates on 2026-09-16; see the generated
[registry status](docs/release-status.md). Paired with the Isotope 2026-09-14
specification snapshot. Historical validation records in
[docs/history](docs/history/) do not certify this revision.

### Added

- Recoverable prepared execution for synchronous and asynchronous host stores,
  with explicit execution owners, joined state recovery, supervisor retention,
  engine-wide epoch cadence, and configurable growth-denial behavior.
- Shared reader/writer capabilities (`SharedReader`/`SharedWriter`) and
  allocation-free normalized suffix matching (`matches_prefix_suffix`).
- Names-only child projections: `ChildNames`, `ChildPage` and
  `Reader::read_children_page` beside the existing `read_children`.
- Explicit file durability (`Durability::Buffered`/`Durability::Synced`) and
  failed-save recovery (`recover`) for the JSON and JSONL file backings.
- Supervised handle release (`SupervisedProtocol`), streaming HTTP and bounded
  SSE framing, structured guest codec diagnostics, and atomic retained-tail
  batches from the Ox supplement.
- Flat-entry `MemoryStore::from_entries` snapshots preserving Null and empty
  containers; optional roots distinguish imported Null from an empty store.

### Fixed

- Handle cleanup joins across opening races and shutdown-hook panics:
  `SupervisedProtocol::open` hands the opened handle to its cleanup task
  through a oneshot channel, so an owner close that starts before `open`
  returns still joins the handle, and a panicking close hook no longer skips
  the join.
- Corrected lazy-decode and masking contracts.

### Changed

- `path!` expands hygienically through renamed dependencies and facade
  re-exports; a direct `structfs-core-store` dependency is no longer required.
- Ordinary Null writes delete. Conventional state assignment translates Null
  to the internal Delete operation.
- Implicit typed reads are parsed-only; explicit server responses have
  canonical presence.
- Legacy driver bridges and public unowned core-Wasm run entry points removed.

See [migration](docs/migration-0.4.md) and
[design decisions](docs/design/2026-09-14-coherent-contracts.md).

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
