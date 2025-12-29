# Plan: Make Meta the Canonical Interface for FsStore

This plan addresses the concision problem identified in the current FsStore
implementation: there are multiple ways to perform the same action. Per
representation.md, "a StructFS interface is concise if there is one and only
one way to perform a given action."

## Current State: The Problem

The FsStore currently has duplicate interfaces:

| Action | Path A | Path B |
|--------|--------|--------|
| Get position | `handles/0/position` → `{"position": N}` | `meta/handles/0/position` → `{"value": N, ...}` |
| Seek | `write handles/0/position {"pos": N}` | `write meta/handles/0/position N` |

The formats differ. The meta path is actually cleaner (bare integer for seek).
But having two ways to do the same thing violates concision.

## Design Principle

Separate **data** from **control**:

- **Data paths** (`handles/0`, `handles/0/at/N`): Access file content
- **Control paths** (`meta/handles/0/position`, `meta/handles/0/close`): Manipulate handle state

The meta lens becomes the canonical interface for anything that isn't raw data
access. This aligns with the meta pattern: "what can I do with this path?"
includes "how do I control it?"

## Target Interface

### Data Paths (No Change to Semantics)

```
handles/                    # List open handles [0, 1, 2]
handles/0                   # Read/write file content at current position
handles/0/at/{offset}       # Read/write at specific offset
handles/0/at/{offset}/len/N # Read N bytes from offset
```

These paths deal exclusively with file content. No control operations.

### Control Paths (Meta)

```
meta/                       # Store schema
meta/open                   # How to open files (read-only, describes the operation)
meta/handles/               # List handles with metadata
meta/handles/0              # Handle affordances and current state
meta/handles/0/position     # Get/set position (THE way to seek)
meta/handles/0/encoding     # Get/set content encoding
meta/handles/0/close        # Close handle (write-only)
```

### Removed Paths

| Path | Reason |
|------|--------|
| `handles/0/position` | Moved to `meta/handles/0/position` |
| `handles/0/meta` | Confused with meta prefix; use `meta/handles/0` for affordances, keep file metadata accessible another way |
| `handles/0/close` | Moved to `meta/handles/0/close` |

### The `handles/0/meta` Question

Currently `handles/0/meta` returns file metadata (size, is_file, is_dir, path).
This is suffix-pattern meta, which the meta.md document distinguishes from
prefix-pattern meta. But having both is confusing.

Options:

**A. Remove suffix meta entirely.** File metadata is available via `stat`.

**B. Rename to avoid confusion.** `handles/0/info` or `handles/0/file` for file
properties.

**C. Keep both, document the distinction.** Suffix = data properties, prefix =
path affordances.

Recommendation: **Option A**. The `stat` operation already provides file
metadata. Having it on handles too is redundant. If users need file info for an
open handle, they can use `meta/handles/0` which includes the file path, then
`stat` that path.

## Detailed Changes

### 1. Simplify `meta/handles/0/position`

Current read response:
```json
{
  "readable": true,
  "writable": true,
  "type": "integer",
  "value": 1024,
  "description": "Current byte offset. Write to seek."
}
```

Proposed read response:
```json
1024
```

Just the integer. The schema information moves to `meta/handles/0` (the parent).

Write accepts: bare integer `1024`

This follows the Plan 9 model: position is a file. Read it, get a number. Write
a number, seek.

### 2. Restructure `meta/handles/0`

Current response mixes state and schema. Separate them:

```json
{
  "position": 1024,
  "encoding": "utf8",
  "mode": "readwrite",
  "path": "/tmp/example.txt",
  "fields": {
    "position": {"type": "integer", "writable": true},
    "encoding": {"type": "string", "writable": true, "values": ["base64", "utf8", "bytes"]},
    "close": {"writable": true}
  }
}
```

The `fields` map describes what sub-paths exist and their types. The top-level
fields are the current state. Reading `meta/handles/0/position` returns just
the position value.

### 3. Make `meta/handles/0/encoding` Writable

Currently encoding is set at open time and immutable. Allow changing it:

```
write meta/handles/0/encoding "utf8"
```

This provides runtime control over how content is encoded on read/write.

### 4. Move Close to Meta

Currently:
```
write handles/0/close null
```

Becomes:
```
write meta/handles/0/close null
```

Close is a control operation, not data access. It belongs in meta.

### 5. Remove `handles/0/position` and `handles/0/meta`

These paths become errors (or return null). Position and control live in meta.

### 6. Update Root Listing

`read /` (the fs root) currently returns:
```json
{
  "open": "Write {path, mode} to get handle",
  "handles": "Open file handles",
  "stat": "Write {path} to get file info",
  ...
}
```

This is documentation masquerading as data. Move it entirely to `meta/`:

```
read /       → {"handles": [...], "open": "write-only", ...}
read meta/   → full schema with descriptions
```

Or simpler: `read /` returns minimal structure, `read meta/` returns docs.

## Migration Path

### Phase 1: Add New Paths

- Add `meta/handles/0/encoding` (writable)
- Simplify `meta/handles/0/position` response to bare integer
- Add `meta/handles/0/close`

### Phase 2: Deprecate Old Paths

- `handles/0/position` logs deprecation warning, still works
- `handles/0/meta` logs deprecation warning, still works
- `handles/0/close` logs deprecation warning, still works

### Phase 3: Remove Old Paths

- Old paths return errors
- Update all tests
- Update documentation

## Value Format Consistency

Per representation.md's consistency principle, the format you write should
match the format you read:

| Path | Read | Write |
|------|------|-------|
| `meta/handles/0/position` | `1024` | `1024` |
| `meta/handles/0/encoding` | `"utf8"` | `"utf8"` |
| `meta/handles/0/close` | error (write-only) | `null` |
| `handles/0` | `"base64content..."` | `"base64content..."` |
| `handles/0/at/100` | `"base64content..."` | `"base64content..."` |

No more `{"pos": N}` vs `N` discrepancy.

## Open Questions

### Should `open` Move to Meta?

Currently: `write open {"path": "...", "mode": "..."}` → `handles/N`

This is a control operation (creates a handle). Should it be `write meta/open ...`?

Arguments for: Consistency. All control through meta.

Arguments against: `open` creates something in the data namespace (`handles/N`).
The broker pattern uses write-to-root for similar operations.

Recommendation: Keep `open` at root. It's a factory operation, not handle control.

### Should `stat`, `mkdir`, etc. Move to Meta?

These are operations on the external filesystem, not on StructFS paths. They're
arguably "control" but they don't have corresponding readable state in the store.

Recommendation: Keep at root. They're actions, not introspection.

## Test Changes

Tests to update:
- Remove tests for `handles/0/position` read/write
- Remove tests for `handles/0/meta`
- Remove tests for `handles/0/close`
- Add tests for simplified `meta/handles/0/position` format
- Add tests for `meta/handles/0/encoding` write
- Add tests for `meta/handles/0/close`
- Add deprecation warning tests (Phase 2)

## Summary

| Before | After |
|--------|-------|
| `read handles/0/position` | `read meta/handles/0/position` |
| `write handles/0/position {"pos": N}` | `write meta/handles/0/position N` |
| `read handles/0/meta` | `read meta/handles/0` (state) or `stat` (file info) |
| `write handles/0/close` | `write meta/handles/0/close` |

The result: one canonical way to control handles, through meta. Data paths are
for data only.
