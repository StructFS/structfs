# Protocol

This document specifies the semantics of StructFS operations in Isotope. For
how Blocks serve these operations, see `07-server-protocol.md`.

## StructFS Foundation

Isotope builds on StructFS. All operations are StructFS reads and writes. This
document describes how those operations behave in the Isotope context.

See the StructFS manifesto (`docs/manifesto.md`) and patterns (`docs/patterns/`)
for foundational concepts.

## Operations

### Read

```
read(path) → Result<Value, Error>
```

Read the value at `path`. Returns:

- `Value` — The data at that path (may be null if path exists but has no value)
- `Error` — Operation failed

Following StructFS semantics, a missing path may return null rather than error.
The distinction between "path doesn't exist" and "path exists with null value"
is store-dependent.

### Write

```
write(path, value) → Result<Path, Error>
```

Write `value` to `path`. Returns:

- `Path` — The path written to (may differ from input for append-style stores)
- `Error` — Operation failed

The returned path is significant. For deferred operations:

```
write("/jobs", {task: "process", data: {...}})
→ "/jobs/outstanding/42"
```

The caller then reads from `/jobs/outstanding/42` to get the result.

## Value Types

Isotope uses StructFS Value types:

- **Null** — Absence of value
- **Bool** — true/false
- **Integer** — Signed 64-bit
- **Float** — 64-bit IEEE 754
- **String** — UTF-8 text
- **Bytes** — Arbitrary octets
- **Array** — Ordered list of Values
- **Map** — String-keyed collection of Values

## Blocking Behavior

StructFS operations can block. Whether they do depends on the store:

### Immediate Return

Most operations return immediately:

```
read("/config/timeout") → "30s"  // Immediate
```

### Blocking Read

Some stores block until data is available:

```
read("/iso/server/requests")  // Blocks until a request arrives
```

This is how Blocks receive incoming requests—they block waiting.

### Deferred Operations (Handle Pattern)

For async operations, write returns a handle path:

```
write("/jobs/submit", {task: ...})
→ "/jobs/outstanding/42"

// Later:
read("/jobs/outstanding/42")
→ {status: "complete", result: ...}
```

The runtime is non-blocking internally. "Blocking" from the Block's perspective
means the runtime suspends that Block until the operation completes.

## Consistency

Isotope does not mandate a global consistency model. Each store defines its own:

### Strong Consistency

```
write("/data/x", 1)
read("/data/x") → 1  // Guaranteed
```

### Eventual Consistency

```
write("/cache/x", 1)
read("/cache/x") → null  // Might not see it yet
// ... later ...
read("/cache/x") → 1     // Eventually consistent
```

### Causal Consistency

Operations from the same Block are seen in order:

```
// Block A:
write("/shared/x", 1)
write("/shared/y", 2)

// Block B:
// If it sees y=2, it must see x=1
```

The consistency model is a property of the store, not the protocol.

## Error Handling

Errors fall into categories:

### Path Errors

- **PathNotFound** — No store handles this path
- **InvalidPath** — Path syntax is invalid

### Permission Errors

- **NotReadable** — Path cannot be read
- **NotWritable** — Path cannot be written
- **Forbidden** — Caller lacks capability

### Store Errors

- **StoreError(details)** — Store-specific error
- **Timeout** — Operation timed out
- **Unavailable** — Store temporarily unavailable

### Value Errors

- **TypeMismatch** — Value type doesn't match expectation
- **ValidationFailed** — Value failed schema validation

## References

Following StructFS patterns, values can contain references to other paths:

```json
{
    "user": {"path": "/users/123"},
    "next_page": {"path": "/results/after/abc"}
}
```

A reference is a map with a `path` key. Clients follow references to get the
actual value. This enables:

- Lazy loading
- Pagination
- HATEOAS-style navigation

See `docs/patterns/reference.md` for details.

## Pagination

Collections use cursor-based pagination:

```
read("/users/limit/20")
→ {
    "items": [...],
    "links": {
        "next": {"path": "/users/after/abc123/limit/20"},
        "self": {"path": "/users/limit/20"}
    }
}
```

Clients follow `links.next` to get subsequent pages. See
`docs/patterns/pagination.md` for details.

## Meta Lens

The `meta/` prefix provides introspection:

```
read("/services/cache/meta/users/123")
→ {
    "readable": true,
    "writable": true,
    "type": "cached_user"
}
```

Meta describes what you can do with a path. See `docs/patterns/meta.md`.

## Request/Response in Isotope

When a Block writes to another Block's store, the operation flows through
the Server Protocol:

```
Block A writes to /services/cache/users/123
→ Runtime creates Request{op: write, path: users/123, data: ...}
→ Request delivered to cache Block's /iso/server/requests
→ Cache Block processes, writes Response
→ Runtime delivers Response to Block A
→ Block A's write returns
```

This is transparent to Block A—it just sees a write that eventually returns.
The Server Protocol is the runtime's mechanism, detailed in `07-server-protocol.md`.

## Open Questions

1. **Partial reads**: Should there be a way to read only part of a large value
   (like HTTP Range requests)?

2. **Conditional operations**: Should there be compare-and-swap or
   if-not-modified semantics?

3. **Bulk operations**: Should there be a way to read/write multiple paths
   atomically?

4. **Streaming**: How should streaming data (logs, events) be represented?
   Infinite pagination? Special stream type?
