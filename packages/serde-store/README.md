# structfs-serde-store

Checked Serde conversion and bounded codecs for StructFS Values.

## Usage

`to_value`/`from_value` convert typed data directly. `ValueCodec` selects tagged
JSON, native JSON, CBOR or FlexBuffers with explicit limits. Tagged JSON v1
preserves bytes, full u64 values and non-finite floats. Plain JSON rejects shapes
it cannot preserve. `ExplicitOption` represents null-valued Some explicitly.
Enable `async` for async typed access.

## Version and support

The 0.3 release line supports Rust 1.96+. See the [API documentation](https://docs.rs/structfs-serde-store)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.3.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.

## Detached access

With `async`, `DetachedTypedReader` and `DetachedTypedWriter` return `Send + 'static`
futures that retain no store borrow. `read_typed_detached` requires parsed records;
`read_as_detached` accepts an owned `Arc<dyn Codec>` for raw records. Conversion and
codec errors preserve their categories and bounded diagnostics. Serialization of
borrowed write input completes before constructing the underlying operation.
See the [composition and migration guide](../../docs/migration-0.3.md).
