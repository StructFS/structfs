# Migrating from 0.2 to 0.3

0.2.0 is published. This checkout targets **0.3.0, unreleased**. The new public
`PathPattern` variant and optional HTTP/handles surfaces warrant an incompatible
pre-1.0 release. No persisted records are rewritten automatically.

| Area | Contract and action |
| --- | --- |
| Path macro | Expressions must be `PathComponent`. The macro borrows them, preserving reuse. Add a direct dependency named `structfs-core-store` even when importing `path!` through `structfs`; renamed dependencies are not supported by the expansion. |
| Path construction | `from_validated_components` now validates in release builds too. Invalid strings panic; use `try_from_components` for fallible construction. |
| Path iteration | Iterate component strings with `path.iter()`. Paths are validated refinements of component byte paths; avoid assumptions about their internal storage. |
| Path persistence | Default Serde remains a slash-separated string. Use `path_serde::components` or `path_serde::optional_components` for legacy arrays. Arrays validate each element without splitting or normalization; `[]` is root, `null` is absent for the optional adapter. |
| Pattern matching | `prefix_suffix` still allows an empty middle. `prefix_suffix_with_min_middle(prefix, suffix, 1)` requires an account-instance component. Handle `PrefixSuffixMinMiddle` in exhaustive matches. Matching remains component-wise. |
| Combinators | `into_inner` consumes ReadOnly, Rooted, Masked and Cascade; Cascade returns its pair. `Shared::lock` exposes synchronous access. Detached Cascade requires an explicitly shared fallback, described below. |
| Async traits | `AsyncReader`/`AsyncWriter` borrow the store until completion. `DetachedReader`/`DetachedWriter` return `Send + 'static` futures and release the borrow before polling. They express different concurrency contracts. |
| Typed access | Enable Serde's `async` feature and import `DetachedTypedReader`/`DetachedTypedWriter`. Parsed records need no codec; raw records require an owned `Arc<dyn Codec>` through `read_as_detached`. `read_typed_detached` rejects raw records. |
| Diagnostics | Serde custom errors retain `TypeMismatch` plus bounded text and field/index context. Inspect the structured category, not exact diagnostic wording. `Limits::max_diagnostic_bytes` caps output; custom text capture is capped at 1024 bytes and location capture at 256 bytes. |
| Assembly | Present standard sections must have the documented types. Config/failure keys must name declared blocks. Unknown top-level and block fields fail; only `x-` extension fields are ignored. Per-block config payloads remain application-defined. `wasm` and `artifact` are alternatives, not simultaneous fields. |
| Platform features | Shared Tokio dependencies no longer enable a native executor. Handles defaults retain `SyncBridge`; disabling defaults exposes portable primitives. HTTP without defaults exposes schemas and errors; native stores require `blocking`. See the platform matrix. |
| Terminal results | Inspect guest exit code and optional `last_error`. A declared nonzero exit need not have a trap diagnostic. Inspect assembly shutdown and owner cleanup reports separately. |

## Detached composition

```rust
use structfs_core_store::{path, Cascade, DetachedShared, MemoryStore, Rooted, Shared};
use structfs_serde_store::{DetachedTypedReader, DetachedTypedWriter};

# async fn demo() -> Result<(), structfs_core_store::Error> {
let primary = Shared::new(MemoryStore::new());
let fallback = DetachedShared::new(Shared::new(MemoryStore::new()));
let mut store = Rooted::new(path!("tenant"), Cascade::new(primary, fallback));
let first = store.read_typed_detached::<i64>(&path!("one"));
let second = store.read_typed_detached::<i64>(&path!("two"));
let write = store.write_as_detached(&path!("other"), &3i64);
write.await?;
first.await?;
second.await?;
# Ok(())
# }
```

`DetachedShared` locks only while constructing an operation. A Cascade starts
its fallback only after a primary `Ok(None)`; errors propagate and hits do not
consult fallback. Sharing retains the same fallback after the wrapper is dropped.
ReadOnly never constructs an underlying write. Rooted rebases write results and
rejects results outside its subtree; rejection cannot undo an already accepted
write. Masked preserves absence and errors. Dropping a detached future has the
underlying provider's cancellation semantics; detachment does not imply an
operation is safe to abandon.

## Explicit subscription representation

PathPattern uses externally tagged snake-case Serde:

```json
{"prefix_suffix":["accounts","provider"]}
```

```json
{"prefix_suffix_min_middle":{"prefix":"accounts","suffix":"provider","min_middle":1}}
```

Exact and prefix patterns serialize as `{"exact":"a/b"}` and `{"prefix":"a"}`.
Zero minimum preserves empty-middle matching. Larger minima are supported and
never wrap on arithmetic overflow. Application-specific older pattern formats
still require their own migration.

## Existing 0.2 value boundaries

`value_to_json` is fallible; JSON cannot preserve Bytes or non-finite floats.
Typed conversion is strict: `from_value::<f64>(Value::Integer(1))` fails. In
contrast `serde_json::from_str::<f64>("1")` accepts integer JSON syntax. A typed
read through a StructFS codec goes through Value and retains the strict rule;
choose an explicit application decoder if legacy usage records need coercion.

Empty maps and arrays are values and remain distinct from Null. In convention-
conforming stores, writing Null deletes a subtree and writing a map replaces it;
empty-container storage must not be confused with an absent read (`None`). For
wire and typed shape details, see [the 0.2 migration](migration-0.2.md).

## Executable adoption checks

- [Detached contract tests](../packages/serde-store/tests/detached_contracts.rs)
  exercise parked reads, independent writes, validation and wrapper effects.
- [Portable consumer](../tests/portable/src/lib.rs) compiles an independent
  browser-shared application graph and explicitly enables native HTTP separately.
- [Cancelled HTTP example](../tests/embedding/examples/cancelled_http.rs) exercises
  actual disconnects, late allocation replies, alias-independent release,
  joined producers, retained capacity, and nonzero guest exit.

Run `scripts/check-release.sh` for workspace and extracted-package acceptance.
