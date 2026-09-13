# structfs-core-store

Validated paths, Values, Records, read/write traits and store composition.

## Usage

Use `Reader`/`Writer` for synchronous stores; enable `async` for async traits.
`path!` validates literal components at compile time. `Value` preserves signed
and unsigned integers, bytes and floats; `Record` can forward unparsed bytes.
Mounts and overlays compose stores without adding persistence or transaction guarantees.

## Candidate and support

Version 0.2.0 targets Rust 1.96+. See the [API documentation](https://docs.rs/structfs-core-store)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.2.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
