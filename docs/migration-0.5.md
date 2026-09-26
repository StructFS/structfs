# Migrating to 0.5

0.5 is a breaking cleanup of the 0.4 surface, driven by the
[2026-09-17 code audit](code-audit-2026-09-17.md). The
[CHANGELOG](../CHANGELOG.md) lists every change; this guide covers what callers
must edit. Most changes are mechanical renames that the compiler points at.

## Facade features and modules

Each item now has exactly one path. Feature-gated items live only in their
module, and module names no longer shadow common crates under `use structfs::*`.

| 0.4 feature / path | 0.5 feature / path |
| --- | --- |
| `serde`, `structfs::serde::X`, `structfs::X` | `typed`, `structfs::typed::X` |
| `json`, `structfs::json::{in_memory, value_utils, InMemoryStore}`, `structfs::InMemoryStore` | all removed; use `structfs::MemoryStore`. The durable stores are the new `persist` feature, `structfs::persist::X` |
| `http`, `structfs::http::X` | `net` (schema, portable) or `net-blocking` (stores), `structfs::net::X` |
| `http-streaming`, `structfs::{sse, streaming}` | `net-streaming`, `structfs::net::{sse, streaming}` (`sse` needs only `net`) |
| `sys`, `structfs::sys::X`, `structfs::SysStore` | `os`, `structfs::os::X` |
| `structfs::{LLPath, LLReader, ...}` | `structfs::ll::{...}` |
| `structfs::{PathPattern, matches_prefix_suffix}` | `structfs::pattern::{...}` |
| `structfs::{mount_store, overlay_store, path_trie}` modules | items at the root |
| `structfs::MountConfig` | `structfs_repl::MountConfig` |
| (none) | `handles` feature, `structfs::handles` |

Feature implications changed too. `http-streaming` implied the blocking stores;
`net-streaming` implies only `net` and `async`, so add `net-blocking` if you
also use the brokers or `HttpClientStore`. `http` and `state` implied `serde`;
`net` and `state` no longer imply `typed`, so enable `typed` explicitly if you
use `structfs::typed`. `persist` still implies `typed`.

## Core store

- Match `Error::InvalidArgument` for caller mistakes (bad page limits, cursors,
  structural writes into scalars, malformed configs). These were `Conflict` or
  `PathError::InvalidPath` in 0.4; `PathError::InvalidPath` is gone.
- `SharedReader::read` is `read_shared`; `SharedWriter::write` is
  `write_shared`.
- `OnlyReadable::new(s)` becomes `ReadOnly::new(s)` (writes now fail with
  `PermissionDenied`). `OnlyWritable` has no direct replacement: write a small
  wrapper, or mount the store and reach it through
  `OverlayStore::add_redirect(from, to, RedirectMode::WriteOnly, None)`, whose
  reads fail with `PermissionDenied` rather than returning `Ok(None)`.
  `SubStoreView::new(s, prefix)` becomes `Rooted::new(prefix, s)` — the
  argument order is swapped.
- `OverlayStore`: `add_layer(p, s)` → `mount(p, s)`;
  `add_read_only_layer(p, r)` → `mount(p, ReadOnly::new(r))`;
  `remove_layer` → `unmount`; `layer_count` → `store_count`.
  `add_write_only_layer` (see `OnlyWritable` above), `unmount_subtree`,
  `has_route`, `route_count` and `mounts` are removed, and `RouteTarget` is
  crate-private. `overlay_store::Store` is now a re-export of `structfs::Store`.
- A custom `StoreFactory` declares `type Config` and implements
  `config_from_value`/`config_to_value`; `create` takes `&Self::Config`.
- `MountStore::mount` rejects an existing name (after path normalization) with
  `Conflict` instead of replacing it; unmount first. `unmount` of a missing name
  is `NotFound`. `MountStore::list_mounts` returns
  `Vec<(String, Option<F::Config>)>`; `MountInfo` is gone.
- `LazyRecord::into_record` returns `Result`.
- `Format` is `#[non_exhaustive]`: replace `Format(cow)` literals with the
  constants, `Format::new(s)`, `Format::from_static(s)` or `From`.
  `PathPattern`, `RedirectMode` and `CodecOperation` are `#[non_exhaustive]` too;
  add a wildcard arm to exhaustive matches.
- `PathComponent::encode` passes numeric strings through. Names written by 0.4
  as `_N_42` still decode to `"42"`; to migrate, list children, `decode` each,
  and rename to `PathComponent::encode(&decoded)` where they differ.

## Serde and persistence

- `Profile` is `CodecProfile`. `ValueCodec::canonical()` returns `Result`, and
  the `require_canonical` field is private: set it with `canonical()`, read it
  with `requires_canonical()`.
- `write_as*` is `write_typed*`; `read_json*`/`write_json*` are removed. For a
  `serde_json::Value`, `read_typed::<serde_json::Value>(path)` works on stores
  that return parsed records (it is parsed-only); for stores that may return
  raw bytes use `read_as::<serde_json::Value>(path, Arc::new(JsonCodec))`, or
  convert a `Value` with `value_to_json`. Pass codecs as `Arc<dyn Codec>`.
- serde-store no longer re-exports `PathError`, `Store`, `AsyncStore`,
  `DetachedStore`, `SyncToAsync`, `SyncToAsyncLL`, `AsyncLLReader`,
  `AsyncLLWriter`, `AsyncLLStore`, `AsyncCoreToLL` or `AsyncLLToCore`; import
  them from `structfs-core-store` (the last two are removed outright).
- Build `Limits` with `Limits::default().with_max_depth(n)` and friends.
- `InMemoryStore` and `value_utils` are removed; use `MemoryStore` and
  `Value::{get, set}`. `BackedStore::root()` returns `Option<&Value>`.
- File backings write the tagged Value JSON v1 envelope. 0.5 reads 0.4 files;
  0.4 silently misreads files 0.5 has written (it parses the
  `["structfs-value",1,…]` envelope as a plain 3-element array), so do not
  roll back a data directory to 0.4. Raise limits for large legacy files with
  `JsonFileBacking::new(path).with_limits(limits)`.
- `LogStore` rejects appending Null with `InvalidArgument`.
- namecode (MSRV 1.96): `encode` is not idempotent, so never re-encode an
  encoded name; use `encode_forced` when a guaranteed `_N_` form is needed.
  `decode` is strict: it rejects uppercase digits and any non-canonical input
  (`DecodeError::NonCanonical`) that 0.4 accepted.

## Handles, service, state, profiles

| 0.4 | 0.5 |
| --- | --- |
| `Owner::cancel()` | `Owner::close()` |
| `Owner::close(t).await` | `Owner::join(t).await` |
| `Owner::join().await` | `Owner::join_indefinitely().await` |
| `OwnerHandle::{cancel, close(t)}` | `OwnerHandle::{close, join(t)}` |
| `Registration`/`OwnedResource` `release`, `close(t)` | `close`, `join(t)` |
| `OwnedTail::release` | `OwnedTail::close` |
| `CleanupSupervisor::close(t)` | `CleanupSupervisor::join(t)` |
| `BlockingStore::close().await` | `join(t).await` (closes, then waits; returns `bool`) |
| `DuplexStream::release`, `StreamReadiness::released` | `close`, `StreamReadiness::closed` |
| `Session`/`OperationHandle` `release` | `close`, then `join(t)` |
| `StateHandle::release` | `close().await` |
| `BudgetAdmission { budget, key: "k".into() }` | `BudgetAdmission::new(budget, "k")` |
| `Router::new(..)` returning `Arc` | `Router::shared(..)` (or `Arc::new(Router::new(..))`) |
| `CallBudget::new(..)` returning `Arc` | `CallBudget::shared(..)` |
| `State::new(..)` | `State::shared(..)` |
| `structfs_service::CancelToken` | `structfs_handles::CancelToken` |
| `structfs_profiles::Token` | `structfs_state::Token` |
| `SyncBridge`, `sync-bridge` feature | removed; drive futures with your runtime |
| `TailLog`/`TailPage`, handles `ByteStream` | `service::OwnedTail`, `DuplexStream` |

`close()` requests cleanup without blocking and `join(timeout)` waits, with
two exceptions: `StateHandle::close` is `async` and returns
`Result<(), ClientError>` (it writes the release to the provider) and there is
no `StateHandle::join`; `BlockingStore::join(t)` returns `bool` (whether
in-flight work finished) rather than a `CloseReport`. `DuplexStream::close`
has nothing to join.

Constructors return `Self` (inside the same `Result`, where they are fallible)
instead of `Arc<Self>`: `Router::new`, `CallBudget::new`, `Profiled::new`,
`OperationHandle::start`, `HeadlessHost::open` (returns `Session`).
`DuplexStream::pair(capacity)` returns `Result<(Self, Self), Error>` (zero
capacity is `InvalidArgument`); wrap an endpoint in `Arc` to call `store()`,
which takes `self: &Arc<Self>`.

Construct `#[non_exhaustive]` limit and schema types with `Default` plus
`with_*` setters or their `new` constructors instead of struct literals.

Removed from service: `RetainedBytes` and `Lease::strong_count`. On the wire,
a released supervised handle reads `Ok(None)`, a `HandleStore` write to a path
that is not a handle path (or below an unknown handle) is `NotFound`, and
`StreamReadiness` serializes `closed` instead of `released`.

State handle ids are random decimal `u64`s and are visible only to the view and
owner that opened them. Give each block or tenant its own `state.view()` (or
bind clients with `owned_by`) for isolation.

`StateLimits::handles` is now a per-principal budget (each view, or each
owner-bound client); the new `max_handles` (default 256) is the global
ceiling. A deployment that used `with_handles(n)` as a total cap should also
set `with_max_handles(n)`. `ClientError`, `Request` and `Projection` are
`#[non_exhaustive]`: add a wildcard arm when matching `ClientError`, and build
`Request` with `Request::new`.

`Client` no longer implements the async/detached reader traits; call its
inherent methods or use it as `SharedReader`/`SharedWriter`.

## HTTP

| 0.4 | 0.5 |
| --- | --- |
| `AsyncHttpBrokerStore` | `BackgroundHttpBrokerStore` |
| `HttpExecutor` (`Result<_, String>`) | `BlockingHttpExecutor` (`Result<_, Error>`) |
| `ReqwestExecutor` | `BlockingReqwestExecutor` |
| `Method::from(http::Method)` | `Method::try_from(http::Method)` |
| REPL mount `asynchttpbroker` | `backgroundhttpbroker` (old tag still accepted) |

Construct `HttpRequest`/`HttpResponse` with `new` and builders. `HttpRequest`
rejects unknown fields. `Error::Http` (the `reqwest::Error` variant) exists
only with the `blocking` or `streaming` feature; gate matches on it. Unknown
handles read `Ok(None)` in both brokers; a non-Null write to an unknown handle
is `NotFound`, while Null is an idempotent no-op. Broker root writes must parse
as an `HttpRequest`; one that does not is `InvalidArgument` (0.4: a JSON
decode `Codec` error). At an `HttpClientStore` root only, a map with a `method`
or `path` key is a request and anything else is POSTed; to POST a body that
has its own `method` or `path` key, wrap it:
`{"method":"POST","body":{...}}`.

## Featherweight

- Build a runtime from configuration: `RuntimeConfig::new(handle)`, then
  `with_timeout`, `with_metering`, `with_transcripts`, `register_builtin`,
  `register_loader`, …, then `Runtime::new(config)`. `register_builtins` and
  `featherweight_component::register` take `&mut RuntimeConfig`.
- `HostStore` is async-only: `store.read(path).await`, `store.write(..).await`.
- Replace `BlockCell` access with `BlockView` getters (`state`, `last_error`,
  `exit_code`, `status_value`, `usage`, `wait_terminal`, …).
  `AssemblyInstance::cell(name)` returns `Option<BlockView>`;
  `public_cell()` and `Namespace::cell()` return `BlockView`.
- These are now crate-private: `BlockCell`, `BlockEvent`, `ServerRequest`,
  `GrantStore`, `SessionLog`, `Target`, `WiringTable`. `SessionBudgetSnapshot`
  and `SessionUsage` are removed along with `SessionBudget`.
- `Metering` has only fuel: `Metering::with_fuel(n)` (or
  `Metering::default()` for unbounded). The epoch interval belongs to the
  engine: `CoreWasmEngine::with_epoch_interval(compile_parallelism,
  max_sessions, memory_limit, interval) -> Result<Arc<CoreWasmEngine>>` and
  `ComponentEngine::with_epoch_interval(interval) -> Result<Arc<ComponentEngine>>`.
  Construct `ExecutionPolicy` with
  `ExecutionPolicy::default().with_fuel(..).with_deadline(..)`.
- Prepare core-wasm code with `engine.prepare(bytes).await?`
  (`prepare(self: &Arc<Self>, bytes: Vec<u8>)`); `CoreWasmBlock::from_file` is
  gone. `RuntimeConfig::register_core_artifact` takes an `Arc`:
  `config.register_core_artifact(name, Arc::new(engine.prepare(bytes).await?))`.
  `CoreWasmBlock::manifest()` returns `&[u8]` instead of `Result<Vec<u8>>`.
- The public `featherweight_component::WasmBlock` (with `new`, `from_file`,
  `manifest` and a sync `run`) is removed. Inspect a component with
  `ComponentEngine::new()?.prepare(&bytes)?.manifest()`.
- Run components by registering a loader:
  `featherweight_component::register(&mut config)`, or
  `config.register_loader(Arc::new(ComponentLoader::new()))` /
  `ComponentLoader::with_engine(engine)` to share a `ComponentEngine`.
  `ComponentLoader` is no longer a unit struct, so write
  `ComponentLoader::new()`. `ComponentEngine::prepare` returns a
  `PreparedComponent`, which exposes only `manifest()`.
- Match the new `RuntimeError` variants (`Admission`, `Policy`, `EngineConfig`,
  `AlreadyJoined`, `ExecutionLost`, `HostPanic`) instead of
  `Wasm { operation }` strings. A failed compile or component worker is now
  `HostPanic`/`ExecutionLost`, not `Wasm { operation: "compile task" }`.
- `ExecutionOwner::cancel()` is `close()`: it requests cleanup (cancelling
  the run) without blocking; `join()`/`wait(t)` wait and return the host.
  `ExecutionScope::cancel` and `AssemblyRequest::cancel` are unchanged — they
  cancel in-flight calls and own nothing to close.
- `AssemblyInstance` is `#[non_exhaustive]`; `featherweight_component::
  WasmBlockState` is private.
- wasi: build `OpenFlags` with
  `OpenFlags::default().with_write(true).with_create(true)` instead of a
  struct literal.
- Guests: status -10 is invalid argument (0.4 SDK guests see it as an unknown
  error); build the reference guest with `--features reference-guest`.
  `state::Handle::release()` is `close()`. `sdk::{ValueError, HostError,
  CodecDiagnostic}` and `state::Error` are `#[non_exhaustive]` (add wildcard
  arms; `HostError` comes from `HostError::from_diagnostic`).
- `shutdown(timeout)` is one deadline for the whole tree; pass a non-zero
  timeout if you expect children to finish gracefully.

## sys and REPL

- `write random/bytes` is now `read random/bytes/{n}` (returns bytes).
  `random/u64` and `time/monotonic` return unsigned integers.
- Env writes no longer change the process environment.
- Use `SysStore::rooted(dir)` / `FsStore::rooted(dir)` to confine filesystem
  access; `SysStore::new()` remains unconfined.
- Unknown open modes and encodings are `InvalidArgument`. Handle ids are
  numbered per store (0.4 shared one process-wide counter), and a store holds
  at most `DEFAULT_MAX_HANDLES` (256) open handles; beyond that `open` is
  `ResourceLimit`.
- `write fs/stat` (and the new `fs/readdir`) returns a `results/{id}` path;
  read it for the answer. 0.4's `stat` only checked the path existed and
  returned the request path. At most `DEFAULT_MAX_RESULTS` (256) answers are
  retained, oldest evicted first. `random/bytes/{n}` is capped at
  `MAX_RANDOM_BYTES` (1 MiB).
- `structfs_repl::run()` became `run(edit_mode: Option<EditMode>) ->
  Result<ExitReason, IoError>` (0.4: `run() -> std::io::Result<()>`).
- `{"type":"structfs"}` mounts are removed; `local` takes a JSON file path.
- repl's public data types (`DefaultMount`, `DocsManifest`, `RedirectInfo`,
  `HelpStoreState`, `CommandSpec`, `InputLine`, `Output`, `PromptConfig`) and
  enums (`CommandResult`, `ContextError`) are `#[non_exhaustive]`.
