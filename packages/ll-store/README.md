# structfs-ll-store

Low-level byte paths and byte payloads for transport, FFI and forwarding adapters.

## Usage

`LLReader`, `LLWriter`, `LLStore` and `LLError` form the transport boundary.
Enable `async` for async trait variants. This layer does not validate paths or
interpret Value semantics or serialization formats.

## Version and support

0.4.0 is the published release; this checkout is the 0.5 development line.
Both support Rust 1.96+. See the [API documentation](https://docs.rs/structfs-ll-store)
for compiled examples and full contracts.
Read the [0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)
(or the [0.4 guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) from 0.3) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
