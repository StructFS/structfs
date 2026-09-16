# structfs-handles

Handle ownership, cancellation and streaming primitives for deferred operations.

## Usage

`HandleStore` implements the `outstanding/{id}` lifecycle via `HandleProtocol`.
`TailLog` provides atomic items-and-terminal-status reads. `Gate` and `CancelToken`
support parked reads. Use `DuplexStream` for bounded consuming binary streams;
append-only `ByteStream` has different retention semantics. The `conformance`
module validates custom handle implementations.

## Version and support

The 0.4 release line supports Rust 1.96+. See the [API documentation](https://docs.rs/structfs-handles)
for compiled examples and full contracts.
Read the [migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.

Disable default features for portable cancellation, gates, streams and handle
stores. `sync-bridge` (default) adds the native blocking bridge's Tokio runtime
requirement; it does not require the multi-thread executor. See the
[platform matrix](../../docs/platforms.md) for tested combinations.

Explicit Null-write release makes a handle inaccessible immediately, requests
`HandleProtocol::close` once, then awaits `close_wait`. Repeated releases await
pending cleanup. Asynchronous cleanup must outlive abandoned wait futures; use
`structfs_service::SupervisedProtocol` to connect it to an existing cleanup owner.
The default close_wait is only for synchronous cleanup. Final store Drop requests
cleanup; the supervisor must remain alive through drain.
