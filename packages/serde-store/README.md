# structfs-serde-store

Checked Serde conversion and bounded codecs for StructFS Values.

## Usage

`to_value`/`from_value` convert typed data directly. `ValueCodec` selects tagged
JSON, native JSON, CBOR or FlexBuffers with explicit limits — pick one with
`CodecProfile` (so named because `structfs-profiles` uses `Profile` for
capability contracts). Tagged JSON v1 preserves bytes, full u64 values and
non-finite floats. Plain JSON rejects shapes it cannot preserve.
`ExplicitOption` represents null-valued Some explicitly. Enable `async` for
async typed access.

Only the tagged JSON profile has a canonical form, so
`ValueCodec::canonical()` returns `Result`: it refuses any other profile at
construction rather than handing back a codec whose decode can never succeed.

`Limits` is `#[non_exhaustive]`; build one by adjusting the default:
`Limits::default().with_max_depth(8).with_max_input_bytes(4096)`.

## Typed access

One operation matrix in three flavours:

| Operation | Sync | Async | Detached |
|---|---|---|---|
| Read, parsing raw records with a codec | `read_as` | `read_as_async` | `read_as_detached` |
| Read, parsed records only | `read_typed` | `read_typed_async` | `read_typed_detached` |
| Write | `write_typed` | `write_typed_async` | `write_typed_detached` |
| Read children, typed | `read_children_typed` | — | — |

Only the sync flavour enumerates children, because only `Reader` has
`read_children`. There is no codec-taking write in any flavour: a typed write
always produces a parsed record.

Every codec-taking method takes an `Arc<dyn Codec>`. A detached operation
outlives the call that created it and so cannot borrow one; since
`Codec: Send + Sync`, the same handle works for all three flavours.

## Version and support

0.4.0 is the published release; this checkout is the 0.5 development line.
Both support Rust 1.96+. See the [API documentation](https://docs.rs/structfs-serde-store)
for compiled examples and full contracts.
Read the [0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)
(or the [0.4 guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) from 0.3) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.

## Detached access

With `async`, `DetachedTypedReader` and `DetachedTypedWriter` return `Send + 'static`
futures that retain no store borrow. Conversion and codec errors preserve their
categories and bounded diagnostics. Serialization of borrowed write input
completes before constructing the underlying operation.
See the 0.4 [composition and migration guide](../../docs/migration-0.4.md) and,
for the 0.5 renames (`write_as*` → `write_typed*`, `Arc<dyn Codec>`), the
[0.5 migration guide](../../docs/migration-0.5.md).

All implicit typed reads are parsed-only, including synchronous `read_typed`.
Raw JSON is not special: select `JsonCodec` explicitly with `read_as`. See the
[0.4 behavior matrix](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md);
0.5 keeps this behavior and removes `read_json*`/`write_json*`.
