# structfs-core-store

Validated paths, Values, Records, read/write traits and store composition.

## Usage

Use `Reader`/`Writer` for synchronous stores; enable `async` for async traits.
`path!` validates literal components at compile time. `Value` preserves signed
and unsigned integers, bytes and floats; `Record` can forward unparsed bytes.
Mounts and overlays compose stores without adding persistence or transaction guarantees.

## Version and support

The 0.4 release line supports Rust 1.96+. See the [API documentation](https://docs.rs/structfs-core-store)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.

## Construction and shared clients

MemoryStore::from_entries builds snapshots without replaying writes. It preserves
Null and empty containers and rejects duplicate/overlapping explicit paths.
MemoryStore::root returns Option<&Value>; imported Null can be present, while an
ordinary Null write deletes. Store conventions describe exposed behavior, not a
restriction on every internal representation.

With `async`, SharedReader and SharedWriter provide object-safe shared access with
owned arguments and Send + 'static futures. DetachedShared holds its mutex only
while constructing an operation. Acceptance and future-drop semantics remain
provider-specific. matches_prefix_suffix and PathPattern::matches allocate nothing.
