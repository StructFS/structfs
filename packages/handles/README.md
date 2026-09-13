# structfs-handles

Handle ownership, cancellation and streaming primitives for deferred operations.

## Usage

`HandleStore` implements the `outstanding/{id}` lifecycle via `HandleProtocol`.
`TailLog` provides atomic items-and-terminal-status reads. `Gate` and `CancelToken`
support parked reads. Use `DuplexStream` for bounded consuming binary streams;
append-only `ByteStream` has different retention semantics. The `conformance`
module validates custom handle implementations.

## Candidate and support

Version 0.2.0 targets Rust 1.96+. See the [API documentation](https://docs.rs/structfs-handles)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.2.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.
