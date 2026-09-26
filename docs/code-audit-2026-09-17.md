# Code audit — 2026-09-17

Scope: the whole workspace at `4127768`, weighted toward the three Codex
commits `ef02a92`, `c68b417`, `4127768` (127 files, +6,914 / −688). Six
parallel read-only auditors covered core-store; serde/json/facade/namecode;
handles/service/state/profiles; http/sys/repl; featherweight; and workspace
hygiene. Every finding marked **[v]** was re-verified by hand against the
source after the auditor reported it; the rest were read from the auditor's
cited lines but not independently re-checked.

## Headline

- **All gates are green.** `cargo fmt --check`, `clippy --all-features
  --all-targets -D warnings`, `RUSTDOCFLAGS=-D warnings cargo doc`, and
  `cargo test --workspace --all-features` all pass (1,700+ tests, 0 failures,
  23 ignored, all with reasons).
- **0.4.0 is already on crates.io** for all 19 workspace crates plus namecode
  0.1.1 (`cargo search` confirms; `docs/release-status.*` regenerated
  2026-09-16T12:28Z says so, uncommitted). Every human-facing doc still says
  "candidate" / "unpublished". API findings below are therefore **0.5 work**
  (or deprecations), and doc/packaging findings are **0.4.1 work**.
- **The concurrency core is sound.** Gate, CancelToken, owner supervision,
  turnstile, and the `4127768` oneshot handoff were all checked for lock
  ordering, mutex-across-await, and lost wakeups; no defects found there.
- **The public surface grew by accretion, not design.** Roughly 40% of
  core-store's public items and ~10 items across handles/service/state have
  zero non-test callers anywhere in the workspace. Several concepts exist two
  to four times under different names.
- **Real bugs exist behind the green gates**, mostly in pre-Codex code that
  has high line coverage but tests only the happy path.

## Verified critical findings

Ordered by how much they will hurt if left alone.

### Unsound or panicking

1. **[v]** `packages/core-store/src/path.rs:274` — `Path::from_ll_unchecked`
   is a *safe* `pub fn` with debug-only validation; every accessor then goes
   through `from_utf8_unchecked` at `path.rs:74`. Zero external callers.
   Delete it or make it `unsafe fn`.
2. **[v]** `featherweight/runtime/src/namespace.rs:119,128` — the sync
   `Reader`/`Writer` impl for `HostStore` calls `Handle::current().block_on`
   for every non-`Sync` variant, and the runtime itself calls it from async
   context: `session.rs:114` (`inner.log.write` under a std mutex) and
   `transcript.rs:300,370`. A wasm block plus a non-`Sync` session or
   transcript store panics on its first boundary op; with `Sync` it does a
   blocking JSONL write on a Tokio worker while holding the mutex.
3. **[v]** `packages/sys/src/fs.rs:219` — `handles/{id}/at/{o}/len/{n}`
   allocates `vec![0; n]` from an unbounded path-supplied `n`. Same family:
   `time.rs:83` negative `{"ms": -1}` wraps through `as u64`; unchecked
   `as u64` seek at `fs.rs:281,650`.
4. `featherweight/runtime/src/core_wasm.rs:1207-1239` — `run_host_sync` ends
   in `.expect(...)`; a `JoinError::Cancelled` or a panic outside the
   `catch_unwind` window kills the supervised task and `ExecutionOwner::join`
   loses the host state the owned-execution API exists to recover.
5. `namecode/src/encode.rs:104-111` + `bootstring.rs:72` — decoder accepts
   uppercase digits the encoder never emits, so a valid identifier like
   `_N_helloworld__FA0B` trips `debug_assert_eq!` in debug and breaks
   round-trip in release. Reaches StructFS via `PathComponent::encode`.
   Related: `PathComponent::encode("")` yields an invalid empty component
   (`path.rs:358`); `PathComponent::decode` is lossy for raw components that
   legitimately start with `_N_` (`path.rs:364-381`).

### Leaks and DoS surfaces

6. **[v]** `packages/handles/src/handle_store.rs:85-87,152-160,228-237` —
   default `close_complete` is `false` and entry removal happens only inside
   the release future. `HttpBrokerProtocol` and featherweight `SpawnProtocol`
   override neither hook, so a Null-write future dropped before first poll
   leaks a tombstone forever. Default `close_complete` to `true`, or remove
   synchronously when `close_wait` is the default.
7. **[v]** `packages/service/src/handle_protocol.rs:67-80` — after owner close
   during open (the race `4127768` targets), `HandleStore` keeps a zombie
   entry: the store's own `cx.cancel` is discarded for the registration token,
   so the entry is never cancelled, is listed as live, and reads return
   `Err(Cancelled)` rather than the documented `None`.
8. `packages/state/src/provider.rs:409-412,427,466-480` — commands that fail
   validation still mint a `Handle` (default cap 64) and an owner
   registration; 64 malformed batches lock out every other view with
   `Overloaded` for 60 s. Also `provider.rs:253,300-327` re-validates and
   canonical-JSON-encodes the whole tree under the single `inner` mutex on
   every mutation.
9. `featherweight/runtime/src/runtime.rs:1086-1127` — `instantiate_scoped`
   inserts every `BlockRuntime` into `ctx.blocks` before `ensure_started` can
   fail, and the error path returns no `AssemblyInstance`, so failed
   instantiation leaks registrations permanently.

### Wrong or unreachable behaviour

10. **[v]** `packages/http/src/core.rs:683-703` — `HttpClientStore` root write
    can never reach its "POST value as body" branch: `HttpRequest` derives
    `Deserialize` with every field `#[serde(default)]` and no
    `deny_unknown_fields` (`types.rs:51-73`), so any map parses as a request
    and `write / {"name":"Bob"}` silently sends `GET <base>`.
11. **[v]** `packages/repl/src/commands.rs:218-230` — `mounts` and `ls` are
    listed in README, completer, highlighter, docs store, and the capture
    allow-list, but the dispatcher has no arm for them.
12. **[v]** `packages/repl/src/store_context.rs:47-62` — `{"type":"http"}` and
    `{"type":"local"}` mounts are refused as "not yet available in new
    architecture" while the http crate docs and REPL help tell users to mount
    them. Three of twelve `MountConfig` variants are dead.
13. **[v]** `packages/sys/src/random.rs:103-110` — `write random/bytes`
    smuggles base64 output through the returned `Path`, so it fails whenever
    the encoding contains `/`, `+`, or `=`; the test asserts "either succeeds
    or fails".
14. **[v]** `packages/json_store/src/persist.rs:83` and `append_log.rs:81,395`
    — persistence serializes `Value` structurally with `serde_json`, so
    `Value::Bytes` round-trips as `Array` and NaN/inf as `null`. The crate
    declares `structfs-serde-store` (which has `ValueJsonCodec` for exactly
    this) and never uses it.
15. `featherweight/runtime/src/protocol.rs:201-202` vs `core_wasm.rs:40-55` —
    typed errors do not survive a block hop: `NotFound`/`NoRoute`/`Path` are
    encoded as `"not_found"`/`"invalid_path"` but decoded back to stringly
    `Error::store`, so `status_of` reports `-9 OTHER` instead of the spec-11
    codes. The test at `protocol.rs:318` skips exactly these variants.
16. **[v]** `packages/core-store/src/overlay_store.rs` and `mount_store.rs` —
    neither routing store overrides `read_children`/`read_children_page`, so
    the 0.4 "names-only discovery" verbs fall back to full `read` + map
    projection above the routing layer and error on `Record::Raw`.

### Published packaging defects (need 0.4.1)

17. **[v]** `packages/structfs/README.md:15-21` — shipped README tells 0.4.0
    users to add `structfs-core-store = "0.3.0" # required by the path!
    expansion`, which `migration-0.4.md:82-84` says is obsolete.
    `featherweight/README.md:13` says `cargo install featherweight --version
    0.3.0` under "Install the 0.4 release line". Both render on crates.io.
18. **[v]** `featherweight/Cargo.toml:15` — `exclude` omits `site/`, so the
    published `featherweight` crate contains 18 Eleventy files
    (`package.json`, `pnpm-lock.yaml`, `.eleventy.js`, `src/**`).
19. **[v]** `tests/applications/conversation-service/src/owned_execution.rs`
    is byte-identical to `featherweight/runtime/tests/owned_execution.rs`
    (501 lines). The "independent consumer" fixture re-runs the runtime's own
    suite.

## Public API: what to fix in 0.5

### Semver traps

- **No `#[non_exhaustive]`** on any public enum or struct in featherweight
  (`error.rs:7`, `block.rs:58,91,102,151`, `namespace.rs:29,264`, …),
  handles, service, state, profiles (`StateLimits` has 14 pub fields,
  `CallLimits`, `Fault`, `Command`, `Profile`, …), serde-store (`Limits` with
  11 pub fields, `Profile`, `ValueCodec`), http (`Method`, `HttpRequest`,
  `HttpResponse`, `RequestState`), or `PathPattern` (`path_pattern.rs:32-43`,
  which changed shape in `ef02a92`). core-store's own `Error`/`Value`/`Record`
  have it; nothing else does.
- `featherweight/runtime/src/metering.rs:69,80,107` — `wasmtime::Config`,
  `Store<T>`, `Engine` in the public API of the "no wasmtime knowledge" core,
  combined with the workspace `wasmtime = "=48.0.1"` exact pin
  (`Cargo.toml:107`) inherited by published libraries.
- `featherweight/runtime/src/block.rs:381,618,639,730` — `BlockCell` is
  reachable via `AssemblyInstance::cell()`/`Namespace::cell()` and exposes
  `set_state`, `respond`, `mark_shutdown_complete`, and `pending_events` (which
  drains the mailbox). Embedders can break every runtime invariant. **[v]**
- `packages/core-store/src/async_traits.rs:185-192` — `SharedReader::read`
  and `SharedWriter::write` reuse `Reader::read`/`Writer::write` names with
  different signatures; types implementing both already force UFCS in the
  crate's own tests.
- **[v]** `packages/core-store/src/traits.rs:134` vs `overlay_store.rs:17` —
  two distinct public traits named `Store`; `StoreBox` and `StoreFactory` are
  typed against the overlay one.
- `packages/http/src/executor.rs:16-21` — `HttpExecutor` returns
  `Result<_, String>`; `AsyncHttpBrokerStore` bypasses it with a bare fn
  pointer (`handle_broker.rs:110`) and duplicates `ReqwestExecutor::execute`,
  so its unit tests hit `https://example.com` live (`core.rs:1741,1780,…`).

### Duplicate concepts

| Concept | Copies |
|---|---|
| In-memory convention store | `core_store::MemoryStore`, `json_store::InMemoryStore` (divergent root/read semantics) |
| Read-only wrapper | `ReadOnly` (PermissionDenied), `OnlyReadable` (Path error) |
| Subtree confinement | `Rooted`, `SubStoreView` |
| Shared/detached store handle | `Shared`, `SyncToAsync`, `DetachedShared`, `Arc<T: SharedReader>`, `service::DetachedProvider` — three poison policies |
| `Client` read families | inherent, `AsyncReader`, `DetachedReader`, `SharedReader` (`service/src/lib.rs:309-529`) |
| Tail page envelope | `TailPage`, `TailRead`, `SnapshotPage`/`ChangePage` |
| Byte stream | `handles::ByteStream`, `http::streaming::ByteStream` (same name) |
| Handle-id convention | `HandleStore` (`u64`), `state::provider` (`h<uuid>`, `provider.rs:261`) |
| "Stop" verb | close / close_wait / cancel / join / release / finish / shutdown / shutdown_write, across 12 types |
| Constructor shape | `Arc<Self>` (`Router`, `State`, `Profiled`, …) vs `Self` (`HandleStore`, `Gate`, …) |
| Value path navigation | `Value::get`, `json_store::value_utils::get_path`, private copies in `http/core.rs:105`, `handle_broker.rs:22`, `store_context.rs:159` |
| Help renderer | `format_help_value_body` and `format_help_value` (`commands.rs:599-1243`, ~650 lines) |
| REPL command table | dispatch, capture allow-list, completer, highlighter, help, docs store — six copies, already disagreeing |
| Broker `read_meta` | sync vs async brokers (`http/core.rs:295-375` vs `860-941`) |
| Error taxonomy `match` tables | `core_wasm.rs:40`, `protocol.rs:140`, `transcript.rs:152`, `session.rs:141`, `wasi/lib.rs:32` |
| `splitmix64`/`fnv1a` | `turnstile.rs`, `determinism.rs`, `transcript.rs` |
| Sync/async wasm plumbing | `deliver`/`deliver_async`, `linker`/`async_linker`, three copies of "instantiate + manifest" in `core_wasm.rs` |

### Dead public API (zero non-test callers, workspace-wide grep)

- core-store: all of `reference.rs` (`Reference`, `TypeInfo`, `TypeDescriptor`);
  `Path::{to_ll_path,try_from_ll_path,validate,from_ll_unchecked}`;
  `Format::{is_json,is_protobuf,is_value}`; `LLToCore`, `AsyncLLToCore`,
  `AsyncCoreToLL`; `Value::normalize`; six `OverlayStore` methods plus three
  `#[deprecated]` shims in a pre-1.0 crate; `RouteTarget`, `MountInfo`,
  `path_serde`. Newly added and unused: `ChildPage`/`read_children_page`,
  `SharedReader`/`SharedWriter`, `matches_prefix_suffix`,
  `MemoryStore::from_entries` (CLAUDE.md tells people to use it), `LazyRecord`.
- handles/service: `SyncBridge` (sole reason the default `sync-bridge` feature
  pulls `tokio/rt`), `TailLog`/`TailPage` (advertised in CLAUDE.md, consumers
  use `OwnedTail`), `ByteStream`/`ByteChunk`, `RetainedBytes`,
  `Lease::strong_count`, `OwnerHandle::child`.
- featherweight: `SessionBudget`/`SessionPermit`/`SessionLimits`,
  `DriverCapabilities`/`DriverControl`, `with_provider_limits`,
  `epoch_interval()`, `provider_owner()`, sync `GrantStore` impls; the
  unprepared `CoreWasmBlock` state and `instantiate()` exist only for
  `#[cfg(test)]`.
- serde-store: `read_json*`/`write_json*` (three flavours), `transcode`,
  `validate_value`, `MultiCodec::with_json`, `ExplicitOption`, `*_with_limits`;
  `write_typed` is documented as "identical to write_as".
- json_store: `value_utils::{get_path_mut,set_path,set_child}`, still
  re-exported by the facade.

### Facade (`packages/structfs`)

- Every serde/json/http/sys item is exported at both `structfs::X` and
  `structfs::<mod>::X` (`lib.rs:80-116`).
- Modules are named `serde`, `json`, `http`, `sys`, so `use structfs::*`
  shadows the external crates of the same name.
- The `json` feature exports the weaker `InMemoryStore` and dead
  `value_utils` but **not** `BackedStore`, `JsonFileBacking`, `LogStore`,
  `JsonlFileBacking` — the reason `json_store` exists.
- `LL*` byte-layer types and `matches_prefix_suffix` sit at the top level;
  `structfs::Profile` (codec profile) sits next to `structfs::profiles`
  (capability contracts); `CborCodec`/`FlexbuffersCodec` are not exported at
  all; `structfs-handles` has no feature or re-export; the feature table omits
  `http-streaming`; the facade forces http's `blocking` default so the
  portable HTTP profile in `docs/platforms.md` is unreachable through it.

### Error typing

CLAUDE.md's "prefer typed variants" rule is not followed at the seams:

- `Error::Conflict` used for caller argument errors (`traits.rs:72,78`,
  `children.rs`, `memory_store.rs:103`, `duplex.rs:45,169,213,254`) **[v]**;
  elsewhere the same class maps to `resource_limit`, `overloaded`, or
  `permission_denied`. Core has no `InvalidArgument` variant.
- `PathError::InvalidPath` is never produced by path parsing; it is a stringly
  error for "cannot set child on non-container" (`value.rs:249`,
  `overlay_store.rs:71`).
- `Error::decode`/`encode` hardcode `CodecErrorKind::Syntax`/`UnsupportedValue`
  (`error.rs:145-159`) and are used for config validation
  (`mount_store.rs:286-366`), which `codec_diagnostic()` then ships across the
  guest boundary as authoritative.
- `http/src/error.rs:37-41` flattens everything, including nested typed core
  errors, into `CoreError::store("http", …)`; timeouts never become
  `DeadlineExceeded`, 401/403 never `PermissionDenied`.
- `RuntimeError::Wasm { operation: &'static str, .. }` carries admission,
  policy, double-join, and engine-config failures distinguishable only by
  string (`error.rs:23-27`, `hosting.rs:90,97`).
- `OverlayStore::read` returns `Err(NoRoute)` for unrouted paths
  (`overlay_store.rs:426-430`), contradicting the `Reader::read` doc, so
  `Cascade<OverlayStore, _>` can never fall through.

## Organization

- **Layering inversion.** core-store's `MountConfig` (`mount_store.rs:28-59`)
  hardcodes `Help`, `Repl`, `Registers`, `AsyncHttpBroker`, `Log`, `Recording`
  variants that only `repl/store_context.rs` interprets, and ~150 lines of
  hand-rolled `config_to_value`/`value_to_config` sit beside the serde derive
  on the same enum. CLAUDE.md's "Adding a new store type" recipe
  institutionalizes this.
- **Four substrate crates that want to be two.** `handles`' `close_wait`/
  `close_complete` hooks exist only so `service::SupervisedProtocol` can plug
  in from above the dependency edge; `service::adapters.rs` duplicates
  `core_store::DetachedShared`; `profiles` is a schema half (justified, used by
  the guest) plus three unrelated host things (`Profiled` belongs in service,
  `HeadlessHost`/`Session` is fixture-grade, `OperationHandle`); `state` rolls
  its own outstanding-handle protocol instead of using `HandleStore`.
- **Runtime carries three generations of design at once**: per-run engines
  (`instantiate()`), the shared engine, and owned execution; `RtCtx` holds 13
  mutex-wrapped config fields so `with_*` builders can mutate through `Arc`
  and silently reconfigure a live runtime (`runtime.rs:144-168`); every `.wasm`
  loaded via the built-in loader gets its own `Engine` plus a 10 ms epoch
  ticker task (`core_wasm.rs:479-480`), defeating the shared-engine design;
  `Metering.epoch` is honoured by the component adapter but ignored by the
  core-wasm binding (`core_wasm.rs:843-851`).
- **Oversized files mixing concerns**: `http/core.rs` (2,451 lines, ~1,400 of
  them tests), `sys/fs.rs` (2,418), `repl/commands.rs` (2,003),
  `core_wasm.rs` (1,779), `runtime.rs` (1,499), `store_context.rs` (1,368),
  `state/provider.rs` (825, with a 240-line `command`).
- **Test duplication**: a `HashMap<Path, Record>` test store is redefined in
  seven core-store files while `MemoryStore` exists; `TestJsonCodec` +
  helpers duplicated in `record.rs`/`lazy_record.rs` (the latter with a
  hand-rolled base64 encoder and tests of the test helper); `MockHost` in
  `repl.rs` duplicates `TestHost`; sync/async broker meta tests are copies;
  namecode repeats four tests in `lib.rs` and submodules; `TestHost` and
  `MockExecutor` are `#[cfg(test)]` so the "testing infrastructure" CLAUDE.md
  advertises is crate-internal.
- **Workspace manifest**: `default-members` is a verbatim copy of `members`
  under a comment saying it excludes the wasm-only guest (which has no
  `cfg(target_arch)` guards and is natively tested anyway). Twelve unused
  dependencies verified across published crates: core-store `thiserror`;
  ll-store `thiserror`; serde-store `structfs-ll-store`, `ciborium`,
  `thiserror`; sys `serde`, `serde_json`, `thiserror`; json_store `serde`,
  `structfs-serde-store`; path-macro `proc-macro2`; featherweight-runtime
  `structfs-ll-store`. CLAUDE.md's "use thiserror" is false for most crates.
  `serde_yaml 0.9` is archived (RUSTSEC-2024-0320). `rand 0.8` in sys drags a
  duplicate `getrandom`.
- **Repo hygiene**: `docs/` mixes living docs with nine dated
  validation/audit session logs and a progress file linking to a sibling
  checkout; `plans/00-03` are complete; `scripts/release_namecode.sh` and
  `scripts/export-featherweight.py` (hard-coded 9-crate list, cannot build)
  are abandoned; `scripts/check-ox-letter.py` and three docs cite files in the
  gitignored `local/`; `tmp/cov.txt` (absolute paths, Feb 2026) and an empty
  `TODO.md` are tracked; root `package.json`/`pnpm-lock.yaml`/`node_modules`
  are agent tooling; `tests/*` carry five private `target/` trees (~6.7 GB)
  and are only run via release scripts, undocumented in CLAUDE.md.

## Docs vs code

- README says 0.3.0 is published; CHANGELOG 0.4.0 lacks the "published"
  marker, uses a flat list instead of Added/Fixed/Changed, and omits the
  `4127768` fix, `read_children`/`ChildNames`, `Durability::Synced`/`recover`.
  `releasing.md`, `plans/02`, `plans/03`, `release-validation-2026-09-15.md`
  all say unpublished. The post-publication checklist in `releasing.md:110-117`
  was not run.
- CLAUDE.md workspace structure omits `service`, `state`, `profiles`,
  `structfs`, `namecode`, and the four featherweight sub-crates; `Value` list
  omits `Unsigned`; clippy command differs from `quality_gates.sh`; the gates
  also enforce 90% coverage, which CLAUDE.md does not mention.
- featherweight README claims epoch intervals other than 10 ms are rejected
  (they are accepted) and disabling epochs is supported (it is ignored);
  "Existing loader and synchronous driver adapters remain supported"
  contradicts `migration-0.4.md`; top-level README says "no deadlock
  detection" and "the shell talks to the terminal directly", both false.
- `sys/README.md` documents mode `create_new`, encodings `latin1`/`ascii`, and
  `fs/readdir`, none of which exist; code accepts `createnew` and `bytes`;
  unknown modes silently fall back to Read; `stat` returns only the path.
- `json_store/README.md` documents `get_path(&tree, &["a","b"])` but the
  function takes `&Path`. serde-store/ll-store doc snippets pin `0.1`/`0.3`.
  `http/src/core.rs:216,751` says `read /outstanding` returns `[0,1,2]`; it
  returns `{items: [refs]}`.
- `docs/migration-0.4.md` spot-check of nine named APIs: all present. Good.

## What is good

- **Path/Value/Record/Error core**: `Path` as a validated refinement of
  `LLPath` is clean; the `path!` macro is hygienic and closes the bare-string
  hole with a compile-fail suite; `MemoryStore::from_entries` overlap check is
  correct; `PathPattern` matching including `checked_sub` edges is correct.
- **serde-store bounded decoding**: every collection, string, key comparison
  and node is charged to one `Budget`; duplicate keys, lone surrogates,
  non-canonical ints, `Some(None)`, `f32` narrowing, and `i128`/`u128` edges
  are handled and tested against normative vectors plus mutation fuzzing. No
  conversion bug found.
- **json_store persistence ordering**: temp+rename, sync-then-dirsync, fence
  until `recover`, partial-tail rejection, each pinned by fault injection.
- **Concurrency primitives**: `Gate`/`CancelToken` enable-before-check;
  consistent lock orders (`Router.routes → Owner.state`, `HandleStore.entries →
  Owner.state`, `CallBudget child → parent`); no mutex across await; all
  `Drop`s that spawn hold a runtime `Handle`. `4127768` is a genuine, correct
  fix. `ExecutionOwner::join`/`wait` are cancel-safe. `turnstile.rs`,
  `determinism.rs`, `transcript.rs`, `session.rs` are small and well-argued.
- **Tests worth keeping**: `owned_execution.rs`, `capacity.rs`,
  `handle_cleanup.rs`, `ownership.rs`, serde-store `tests/value_v1.rs`, http
  `sse.rs` chunk-boundary test, `streaming.rs` against a raw TCP server.
- **Layering rule holds**: `featherweight-runtime` has no WIT/component
  dependency; `featherweight-wasi` depends only on core-store.
- `handle_broker.rs` and `recording_store.rs` are the cleanest files in the
  http/repl area and use typed errors correctly.

## Recommended order of work

### 0.4.1 (patch, no API change)

1. Commit the regenerated `release-status.*`; update README, CHANGELOG,
   `releasing.md`, `plans/02-03` to say 0.4.0 is published; run the
   post-publication checklist.
2. Fix `packages/structfs/README.md` and `featherweight/README.md` install
   instructions; add `site/` and `tests/` to featherweight's `exclude`.
3. Bug fixes that need no signature change: fs `len` cap and negative
   duration/seek rejection (#3); `close_complete` default (#6); zombie entry
   (#7); state fault-path handle minting (#8); registration leak on failed
   instantiate (#9); `HttpRequest` `deny_unknown_fields` or a discriminator
   (#10); implement or delete `mounts`/`ls` (#11); namecode uppercase digits
   and empty input (#5); `run_host_sync` `expect` → outcome (#4); switch
   `JsonFileBacking`/`JsonlFileBacking` to `ValueJsonCodec` (#14, file format
   change, so document it).
4. Drop the twelve unused dependencies; replace the copied
   `owned_execution.rs` with a consumer-level test; delete `tmp/cov.txt`,
   empty `TODO.md`, `release_namecode.sh`, `export-featherweight.py`; move
   dated docs and completed plans to `docs/history/`.

### 0.5 (breaking, do it once)

1. **Delete** `from_ll_unchecked` (#1) and every dead public item listed
   above. Make `BlockCell` mutators `pub(crate)`. Remove `wasmtime` types from
   the runtime's public API and pin `=48.0.1` only in the `fw` binary.
2. **Add `#[non_exhaustive]`** to every public enum and to limit/config
   structs, with builders where struct-update syntax was the intended API.
3. **Add an `InvalidArgument` variant** to core `Error`; stop using
   `Conflict`/`InvalidPath`/`decode` for argument, structural, and config
   errors; make http and featherweight preserve nested typed errors; make
   `OverlayStore::read` return `Ok(None)` or document `NoRoute` in the
   `Reader` contract.
4. **Collapse duplicates**: one in-memory store (`MemoryStore`), one read-only
   wrapper, one confinement wrapper, one shared-handle wrapper with one poison
   policy, one `Store` trait, one `Client` read family, one tail envelope, one
   handle-id convention, one stop-verb vocabulary, one constructor shape.
5. **Move `MountConfig`'s REPL variants out of core-store** and delete the
   hand-rolled config serializers. Make routing stores forward `read_children`
   / `read_children_page` (#16).
6. **Restructure the facade**: one path per item, rename `serde`/`json`/
   `http`/`sys` modules, `LL*` under a submodule, `json` feature exports the
   persistence types, add `handles` feature and a non-blocking `http` option,
   rename codec `Profile`.
7. **Merge `handles` into `service`** and `profiles`' schema into `state`;
   rebuild state's outstanding handles on `HandleStore`.
8. **Runtime**: make transcript/session logging async and remove `block_on`
   from every path the runtime drives itself (#2); one engine model in
   `core_wasm.rs` with the sync/async linker and `deliver` generated from one
   body; frozen `RuntimeConfig` instead of 13 mutexed fields; make
   `HttpExecutor` the single execution seam with typed error mapping.

## Resolution notes (0.5 cleanup)

The 0.5 cleanup (see [CHANGELOG](../CHANGELOG.md) and
[migration-0.5](migration-0.5.md)) acted on most of the list above. These
recommendations were deliberately not followed:

- **Substrate crates not merged.** `handles` is portable (builds for `wasm32`,
  no native scheduling) while `service` needs Tokio's `rt`/`time`; the guest SDK
  enables `state` and `profiles` as separate optional features.
- **`state` not rebuilt on `HandleStore`.** `HandleProtocol::open` is
  synchronous and its `HandleCx` carries no call context, but state handles
  answer only to the opening view/owner principal; the move would also change
  the wire (random decimal ids vs `HandleStore`'s `outstanding/{id}` counter).
- **`Profiled` not moved into `service`.** It is built on `profiles`'
  `Declaration` schema and `serde-store` conversion; `service` depends on
  neither and should not.
- **`OverlayStore` keeps `Err(NoRoute)`** for unrouted paths; this is now
  documented on the type and in the `Reader::read` contract.
- **Per-call owned-wrapper allocation kept.** `Client::call` wraps the provider
  in one `Arc<OwnedService>` per `owned_by` owner per call; removing it is low
  value next to the call itself.
- **`json_to_value` stays infallible.** Every parsed JSON DOM value has a
  `Value` form (an unrepresentable `arbitrary_precision` number falls back to
  the nearest finite `f64` or its literal string).
- **`read_children_typed` is sync-only.** The async and detached reader traits
  have no child enumeration to build it on.
- **namecode `encode` is not idempotent.** Encoding every `_N_`-prefixed input
  is what makes `decode(encode(s)) == s` exact for all strings.
- **`Reference`/`TypeInfo`/`Value::normalize` kept.** They have callers (sys
  and http docs/handle references; the FlexBuffers profile); the dead-API
  finding was wrong.
- **`transcode`/`ExplicitOption` kept.** Both are normative in
  [the Value v1 spec](specs/structfs-value-v1.md).
- **`path_serde` kept.** The `tests/portable` consumer uses
  `path_serde::components`.
- **`OwnerHandle::child` kept.** It is the only constructor for nested owner
  scopes (and the only producer of `ResourceKind::Child`).
