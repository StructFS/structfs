# structfs-ll-store

Low-level byte paths and byte payloads for transport, FFI and forwarding adapters.

## Usage

`LLReader`, `LLWriter`, `LLStore` and `LLError` form the transport boundary.
Enable `async` for async trait variants. This layer does not validate paths or
interpret Value semantics or serialization formats.

## Version and support

The 0.4 release line supports Rust 1.96+. See the [API documentation](https://docs.rs/structfs-ll-store)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
