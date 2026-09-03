# Byte Streams

Structured values cover most StructFS traffic, but `read(2)`-shaped
consumers — file I/O, sockets, stdio, response bodies — need bytes in
ranges, with end-of-stream signaling and reads that can wait for more.
This pattern specifies how a store serves a byte stream through ordinary
paths.

## Paths

Given a stream at some handle path `{h}`:

| Path | Operation | Result |
|------|-----------|--------|
| `read {h}/at/{offset}/len/{n}` | Ranged read | `Bytes` (≤ n) |
| `read {h}/len` | Bytes so far | `Integer` |
| `read {h}/closed` | Terminal state | `Bool` |
| `write {h}/append` `Bytes` | Append | Returns the write path |

The `at/{offset}/len/{n}` shape matches the file-handle paths
`structfs-sys` already serves — one convention for files and streams.

## Read semantics (`read(2)`)

A ranged read **parks** until at least one byte past `offset` exists or
the stream is closed, then returns up to `n` bytes.

- **A non-empty result** may be shorter than `n`; the caller advances
  its offset by the bytes received and reads again.
- **An empty result means end-of-stream** — exactly the `read(2)`
  return-0 contract. Because terminality travels with the read, there is
  no close-out race between a data read and a separate "is it done?"
  read.
- **Offsets past the end clamp**: a stale cursor on a closed stream
  reads empty (EOF), never an error.

Cancellation follows the handle rules: releasing the handle fails parked
reads (`EINTR`-shaped), writes stay open for teardown.

## The offset is the cursor

There is no server-side read position. Every reader tracks its own
offset, so many readers can consume one stream independently, and a
retry re-reads the same range idempotently.

## Reference implementation

`structfs_handles::ByteStream` implements the buffer: `push`/`close` on
the producer side; `read_at` (parking), `read_at_cancellable`, and
`snapshot_at` (non-blocking) on the consumer side. Serve it under a
handle store and the paths above are a thin match statement.
