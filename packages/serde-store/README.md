# structfs-serde-store

Checked Serde conversion and bounded codecs for StructFS Values.

## Usage

`to_value`/`from_value` convert typed data directly. `ValueCodec` selects tagged
JSON, native JSON, CBOR or FlexBuffers with explicit limits. Tagged JSON v1
preserves bytes, full u64 values and non-finite floats. Plain JSON rejects shapes
it cannot preserve. `ExplicitOption` represents null-valued Some explicitly.
Enable `async` for async typed access.

## Candidate and support

Version 0.2.0 targets Rust 1.96+. See the [API documentation](https://docs.rs/structfs-serde-store)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.2.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
