# structfs-service

Native async providers, scoped cloneable clients, component-wise routing, and
shared call admission. This crate does not depend on Featherweight or Wasmtime.

Mount a `Service` with an explicit provider base and admission policy, build a
`Router`, and obtain a `Client`. Client scopes and read/write permissions only
attenuate authority. Returned write paths must remain inside both the provider
grant and the client scope. Unwired paths return permission denied.

`DetachedProvider` releases its dispatch mutex before awaiting provider I/O.
`ImmediateStore` explicitly opts into short synchronous work on the caller's
thread. `BlockingStore` uses Tokio's blocking executor and retains admission
leases until running work finishes, even if the caller cancels. Its `close()`
closes admission and waits for that work; cancellation is not rollback.

`CallContext` propagates request identity, cancellation, and deadlines. Providers
that start work outside their returned future must retain its lease and own and
join that work. Use `CleanupSupervisor`, a unique `Owner`, and cloneable `OwnerHandle`s to
register cleanup before exposing a handle and supervise work with `spawn`.
`close(timeout)` reports remaining work; cancellation is not completion.

`CallBudget` is shared with Featherweight and supports immutable budget ancestry
and live limit changes. The `calls_per_block` and `bytes_per_block` field names
are retained for compatibility; in this generic API they bound each admission
key, which may identify a native provider or client partition instead of a block.
Bytes measure request weight, not RSS or retained response buffers.

`Router::register` adds owned mounts with immediate revocation. `Client::owned_by`
binds calls to a request lifetime without revoking the shared service. Owner
constraints survive client cloning, scoping, and metadata changes.

`RetainedBytes` bounds owned result storage. `OwnedTail` bounds items and bytes,
rejects full writes, pages terminal state atomically, and reclaims acknowledged
entries. Owner close releases retained storage and wakes readers.

Use `OwnerHandle::open` for asynchronous handle creation: it reserves cleanup
before dispatch and joins and releases an opened handle even if its caller
abandons delivery. The opener supplies a host-owned release callback.

Keep the supervisor and Tokio runtime alive through shutdown. Cleanup errors and
panics remain reported and charged until the host reconciles them explicitly.
