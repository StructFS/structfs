# structfs-handles

Handle ownership, cancellation and streaming primitives for deferred operations.

## Usage

`HandleStore` implements the `outstanding/{id}` lifecycle via `HandleProtocol`.
`Gate` and `CancelToken` support parked reads: park until a predicate holds,
with the enable-before-check ordering that makes a racing wakeup impossible to
lose. Use `DuplexStream` for bounded consuming binary streams. The
`conformance` module validates custom handle implementations.

Constructors return `Self`; wrap a value in `Arc` yourself when you need shared
ownership. `close()` requests cleanup without blocking; where there is cleanup
to wait for, `join(timeout)` waits for it. `DuplexStream` has only `close()`:
its buffers are discarded synchronously.

## Version and support

0.4.0 is the published release; this checkout is the 0.5 development line.
Both support Rust 1.96+. See the [API documentation](https://docs.rs/structfs-handles)
for compiled examples and full contracts.
Read the [0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)
(or the [0.4 guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md) from 0.3) before upgrading
code or persisted data. The [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md)
describes package, platform and feature verification.

The crate has no optional features: cancellation, gates, streams and handle
stores are all portable. See the [platform matrix](../../docs/platforms.md)
for tested combinations.

Explicit Null-write release makes a handle inaccessible immediately, requests
`HandleProtocol::close` once, then awaits `close_wait`. Repeated releases await
pending cleanup. Asynchronous cleanup must outlive abandoned wait futures; use
`structfs_service::SupervisedProtocol` to connect it to an existing cleanup owner.
The default `close_wait` is only for synchronous cleanup, and pairs with the
default `close_complete` — a protocol that overrides one must override both, or
the store reclaims the handle's slot before its cleanup has finished. Final
store Drop requests cleanup; the supervisor must remain alive through drain.
