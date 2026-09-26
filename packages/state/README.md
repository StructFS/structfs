# structfs-state

An in-memory revisioned tree with conditional atomic batches, pinned snapshots,
and bounded observation. The default `service` feature provides `State`,
`StateClient`, and owner-bound server handles. Disable default features for the
portable request/reply types used by Wasm guests.

Create `State::shared` under a `structfs-service` owner, then mount
`state.view(base, writable)` to grant a subtree. The view exposes read-only
`data`, versioned writes to `operations`, and opaque `outstanding` handles.
Command paths are relative to the view. Use a request-owned client so abandoned
handles are cleaned up.

Handles belong to the view they were opened through (plus the owner, for a
client bound with `owned_by`). Clients sharing one view without an owner share
its handles by design: the view is the grant. For isolation between blocks or
tenants, call `state.view()` once per block or tenant. Another view's handle
reads as `Closed` and releasing it is a silent no-op, exactly like an unknown id.

Rejected commands return a readable typed fault without using a handle slot.
Each view (and owner) holds at most `limits.handles` faults and the whole state
at most `limits.max_faults`; past either bound the oldest fault is dropped and
then reads as `Closed`, as does one that has aged out or been released.

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
