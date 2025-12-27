# Plan 3: Filesystem Position Abstraction - ✅ COMPLETED

## Status: DONE (2025-12-27)

This plan has been fully implemented. File position is now explicitly tracked and addressable via paths.

## What Was Implemented

### Explicit Position Tracking

From `packages/sys/src/fs.rs:35-45`:

```rust
struct FileHandle {
    file: File,
    path: String,
    mode: OpenMode,
    /// Explicit position tracking (mirrors file.stream_position())
    position: u64,
    encoding: ContentEncoding,
}
```

### Position as Addressable Path

The `HandleOperation` enum (lines 48-62) defines all path-based operations:

```rust
enum HandleOperation {
    ReadToEnd,                              // /handles/{id}
    AtOffset { offset: u64 },               // /handles/{id}/at/{offset}
    ReadAtLen { offset: u64, length: u64 }, // /handles/{id}/at/{offset}/len/{n}
    Position,                               // /handles/{id}/position
    Meta,                                   // /handles/{id}/meta
    Close,                                  // /handles/{id}/close
}
```

### Path Structure

| Path | Read | Write |
|------|------|-------|
| `/sys/fs/open` | - | Open file, returns handle path |
| `/sys/fs/handles` | List open handle IDs | - |
| `/sys/fs/handles/{id}` | Read from current pos to EOF | Write at current pos |
| `/sys/fs/handles/{id}/at/{offset}` | Read from offset to EOF | Write at offset |
| `/sys/fs/handles/{id}/at/{offset}/len/{n}` | Read n bytes from offset | - |
| `/sys/fs/handles/{id}/position` | Get `{"position": n}` | Set `{"pos": n}` |
| `/sys/fs/handles/{id}/meta` | File metadata | - |
| `/sys/fs/handles/{id}/close` | - | Close (write null) |

### Usage Examples

```bash
# Open a file
@handle write /ctx/sys/fs/open {"path": "/tmp/test.txt", "mode": "readwrite"}
# Returns: handles/0

# Read entire file from current position
read /ctx/sys/fs/handles/0

# Read from specific position
read /ctx/sys/fs/handles/0/at/100

# Read 50 bytes from position 100
read /ctx/sys/fs/handles/0/at/100/len/50

# Check current position
read /ctx/sys/fs/handles/0/position
# Returns: {"position": 150}

# Seek to position
write /ctx/sys/fs/handles/0/position {"pos": 0}

# Write at specific offset
write /ctx/sys/fs/handles/0/at/200 "data at 200"

# Close
write /ctx/sys/fs/handles/0/close null
```

## Tests

Comprehensive tests in `packages/sys/src/fs.rs`:
- `handle_position_set` - Write `{"pos": 5}` to seek
- `read_at_offset` - Read from `/at/5`
- `read_at_offset_len` - Read from `/at/2/len/3`
- `position_persists_after_read` - Position updates correctly
- `write_at_offset` - Write to `/at/3`
- `position_query_initial` - Initial position is 0

All passing.
