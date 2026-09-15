# Ox letter response audit — 2026-09-15

The upstream requests in the September 14 letter are implemented. This audit
checked the contracts against current source, exercised additional failure paths,
and tested two substitutions against actual Ox subscription sources. It is not a
claim that Ox's entire application has migrated.

## Request-by-request findings

| Letter request | Contract and evidence | Code Ox can remove; work it retains |
| --- | --- | --- |
| Prepared synchronous hosting with recoverable state | `CoreWasmBlock::start_sync` runs the entire fresh instance on a blocking worker after bounded engine admission. `ExecutionOwner::join` returns the non-Clone host separately from its exit result after supervised work joins. Ten owned-execution tests cover reuse, traps, sync/async panics, accepted effects, queued cancellation/deadlines, independent runs, owner abandonment and growth denial. | Can replace its Wasmtime compilation/instantiation, limits, ticker and host-recovery implementation after adapting the guest ABI and host interface. Effect interpretation and conversation policy remain Ox code. |
| Borrowed suffix matching | `matches_prefix_suffix` compares borrowed components with checked length subtraction. Allocation-count tests cover repeated hits/misses, empty suffixes and extreme minima. Ox's existing matcher tests pass with a single call using minimum 1. | Delete the local suffix algorithm. Keep the persisted enum and component-array Serde adapter if retaining existing records. |
| Shared erased writer | `SharedWriter` has the requested `&self`, owned arguments and Send + 'static future. Ox's subscription trait can become a reexport; its existing mock implementations and subscription tests compile unchanged. `DetachedShared` releases its lock before polling. | Delete the local trait declaration. Keep dispatch ordering, post-write hooks and cascade limits. |
| Explicit snapshot import | `MemoryStore::from_entries` preserves present Null, empty maps and empty arrays. Validated Paths enforce component grammar. Duplicates and explicit ancestor/descendant conflicts fail in either order. | Replace unambiguous flat-entry construction. Ox must choose how to handle its old duplicate-overwrite behavior, malformed overlaps, and flat-store enumeration expectations. |
| Typed-record parity | All implicit typed helpers reject raw records through NoCodec. Parsed/Null/absent and raw JSON/other-format results agree in the parity fixture. Migration documentation contains the requested table. | The explicit NoCodec workaround is unnecessary for parity; explicit JsonCodec remains available when decoding is intended. |
| Path macro migration | Expansion uses a hygienic core/facade reexport. The portable consumer tests renamed dependencies on Wasm. Migration notes cover reexporting PathComponent and propagating PathError. | No duplicate validated component type or direct-dependency workaround is needed. Application newtypes can remain if meaningful. |
| Release status | Registry status is generated separately from validation and identifies coordinated 0.4 packages as unpublished. | Downstream readiness can use the exact-version record; passing tests do not imply publication. |

Snapshot construction is distinct from an exposed store write. Isotope-facing
Null writes follow the deletion convention; internal state and imported data may
represent present Null. Nothing in this audit adds a universal deletion law.

## Corrections made during this audit

`DetachedShared` previously rejected a poisoned construction lock through the
shared interface but recovered the same lock through the detached interface.
Both now reject subsequent calls without reentering the possibly inconsistent
provider. The regression test poisons the lock through both reads and writes,
then checks both interfaces and cloned handles.

Provider acceptance is now concrete in the adapter and Client documentation.
DetachedShared constructs the underlying operation immediately and inherits its
acceptance/drop semantics. Client futures are lazy: unpolled drops dispatch
nothing; pending drops signal cancellation and drop the provider future, without
rollback. Providers retaining work retain the context lease until completion.

The additional runtime tests hold a synchronous write outstanding, queue 32
other executions, cancel them, recover their untouched hosts, and expire another
queued execution while the unrelated host remains blocked. After cancellation,
the accepted write finishes and a second import never dispatches. Reusing the
recovered host verifies released engine and supervisor capacity. A separate async
panic test verifies recovery of an accepted effect and capacity reuse.

## Actual downstream source probe

Inspected Ox HEAD: `2bcc9e635da08331dc3c13cdd217f852afc2da8f`.
The probe reads the working-tree files and records their hashes, so local edits
are distinguishable from that commit. It copies only `horns-core`'s
`subscription.rs`, `path_serde.rs` and `write.rs` into an isolated crate, replacing:

```rust
// Local trait definition becomes:
pub use structfs_core_store::SharedWriter as AsyncWriter;

// Local PrefixSuffix matching body becomes:
structfs_core_store::matches_prefix_suffix(path, prefix, suffix, 1)
```

All 28 existing tests pass unchanged, including subscription matching, serialized
patterns, component-array paths and shared writer use. The probe does not edit Ox
or copy its implementation into a published StructFS package. Reproduce it with:

```sh
python3 scripts/check-ox-letter.py /path/to/ox
```

The script emits the exact migration diff, original file hashes, copied-source
crate and test log under `local/ox-letter-*`. It uses offline Cargo resolution
seeded from this candidate's lockfile. This optional downstream probe is separate
from the release gate, whose independent consumers need no Ox checkout.

## Verified downstream boundaries

Source inspection used `rg` and direct reads of these Ox files:

- `crates/ox-runtime/src/engine.rs:202`: `run_with_cancellation` returns the
  HostStore separately from execution failure. Its imports still use the `ox`
  ABI, so existing guest binaries cannot simply be passed to Featherweight.
- `crates/ox-runtime/src/host_store.rs:44`: host operations are exposed as
  `handle_read` and `handle_write`. Ox can implement Reader/Writer by forwarding
  to these methods, retaining backend/effects ownership and interception policy.
- `crates/horns-core/src/subscription.rs:74`: the local suffix matcher requires
  at least one middle component; `:181` defines the matching shared writer shape.
- `crates/ox-store-util/src/local_config.rs:24`: construction inserts into a flat
  map and overwrites duplicate keys. MemoryStore import intentionally rejects
  ambiguous input instead of silently selecting a winner.
- `crates/ox-broker/src/sync_adapter.rs:42`: the synchronous typed helper
  explicitly selects NoCodec to match its detached facade.

The remaining guest migration, tool-cancellation mapping, renderer adaptation,
and production rollout are the downstream responsibilities named in the letter.
The upstream hosting contract is exercised with real Wasm guests and independent
consumer tests; the complete Ox conversation runner was not migrated or tested
here. This audit found no further upstream blocker within the letter's scope.
