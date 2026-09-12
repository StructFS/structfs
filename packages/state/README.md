# structfs-state

An in-memory revisioned tree with conditional atomic batches, pinned snapshots,
and bounded observation. The default `service` feature provides `State`,
`StateClient`, and owner-bound server handles. Disable default features for the
portable request/reply types used by Wasm guests.

Create `State` under a `structfs-service` owner, then mount `state.view(base,
writable)` to grant a subtree. The view exposes read-only `data`, versioned writes
to `operations`, and opaque `outstanding` handles. Command paths are relative to
the view. Use a request-owned client so abandoned handles are cleaned up.

`observe` creates a pinned snapshot and watch cursor atomically. Changes carry
commit tokens and conservative subtree invalidations. Pages advance across
filtered commits, and expired history returns a typed `CursorExpired` fault.
Full history evicts old records without blocking writers. Batches that cannot
fit an atomic change record are rejected before commit.

Snapshots page preorder nodes, including empty container skeletons and arbitrary
map keys. `StateHandle::projection` materializes a bounded immutable `Reader` for
synchronous rendering. Snapshot/handle storage, age, and page sizes are bounded.
Release is explicit and idempotent; owner cleanup and expiry handle abandonment.

This provider promises memory durability only. Cancellation after commit does not
undo a batch, and clients never automatically retry a potentially committed write.
