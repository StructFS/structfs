# Bounded consuming streams

`structfs_handles::DuplexStream::pair(capacity_per_direction)` creates two
endpoints with independent receive and transmit buffers. Consumed bytes are
reclaimed. This is a transport primitive; `ByteStream` remains the append-only,
cursor-addressed history primitive.

Each endpoint is held by `Arc`. The last owner releases it, clears both queues,
and wakes both ends. Explicit `release()` has the same effect. `shutdown_write()`
only closes transmit: the peer drains queued bytes and then receives EOF, while
reverse traffic remains possible.

Reads require a positive maximum. They park until data, EOF or release. An empty
successful read means EOF. Writes transfer one whole chunk atomically, park when
there is insufficient room, and reject a chunk larger than capacity. Cancellation
of a pending operation transfers nothing. A successful operation is not undone by
later cancellation. Queue capacity bounds retained data, not buffers owned by
callers waiting to write; hosts must also bound concurrent operations and handles.

Readiness is advisory and reports readable/writable bytes, EOF, write-closed and
released flags. A readiness wait takes read/write interests and a cancellation
token. Use `select!` to multiplex it with control events, and prefer cancellation
when multiple branches are ready. There is no thread per connection.

The endpoint's `store()` provides a `DetachedStore` for an explicitly granted
mount or handle:

| Operation | Path | Value/result |
| --- | --- | --- |
| read | `rx/{max}` | Bytes; empty at EOF |
| write | `tx` | Bytes, atomic chunk |
| read | `ready/read`, `ready/write`, `ready/both` | Parked readiness map |
| write | `shutdown` | Null: half-close transmit |
| write | root | Null: release both directions |

Dropping a pending detached future cancels just that operation. A handle provider
must call `release()` when releasing its handle: an in-flight future also holds an
endpoint reference. Providers own connection admission and join their socket
pump tasks before releasing reservations. The stream primitive does not open
sockets or grant network authority. Binary console services can use the same
contract, with stdin/stdout/stderr wired independently.
