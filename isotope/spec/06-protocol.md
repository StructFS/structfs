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

## The StructFS Data Model

StructFS defines a **semantic data model**, not a serialization format. This is
analogous to Rust's serde: serde defines an abstract data model that many
concrete formats (JSON, TOML, MessagePack, bincode) can serialize to and from.

### Value Types

The StructFS data model comprises:

- **Null** — Absence of value
- **Bool** — true/false
- **Integer** — Signed 64-bit
- **Float** — 64-bit IEEE 754
- **String** — UTF-8 text
- **Bytes** — Arbitrary octets
- **Array** — Ordered list of Values
- **Map** — String-keyed collection of Values

### Serialization Independence

Any serialization format that can faithfully represent these types IS StructFS-
compatible:

- **JSON** — Maps naturally to the data model
- **Protocol Buffers** — StructFS-compatible when schema maps to Value types
- **MessagePack** — Binary format, fully compatible
- **CBOR** — Binary format, fully compatible
- **Custom binary formats** — Compatible if they preserve semantics

When a Block reads from a path, it receives a Value—not "JSON" or "protobuf."
The bytes on the wire might be any compatible serialization. The Block operates
on the semantic Value, not the encoding.

```
                     StructFS Data Model
                     (semantic contract)
                            │
          ┌─────────────────┼─────────────────┐
          │                 │                 │
        JSON            Protobuf          MessagePack
     (encoding)        (encoding)         (encoding)
```

### Block Serialization Declaration

A Block's serialization format is declared in the Assembly definition, before
the Block starts. All communication with the Block—including startup and
shutdown—uses this format. There is no negotiation; the format is fixed for
the Block's lifetime.

```yaml
# In Assembly definition
blocks:
  api:
    wasm: ./api-block.wasm
    serialization: application/json

  backend:
    wasm: ./backend-block.wasm
    serialization: application/protobuf
```

Common formats:

- `application/json` — JSON encoding
- `application/protobuf` — Protocol Buffers encoding
- `application/msgpack` — MessagePack encoding
- `application/cbor` — CBOR encoding

The runtime uses this declaration to encode all Values delivered to the Block
and decode all Values the Block writes. The Block sees only bytes in its
declared format—it never needs to handle format detection or switching.

### Runtime Translation

When two Blocks with different serialization formats communicate, the runtime
translates:

```
Block A (JSON) writes to Block B (protobuf)
→ A writes JSON bytes
→ Runtime decodes JSON to Value
→ Runtime encodes Value to protobuf
→ B receives protobuf bytes
```

This translation is transparent. Block A thinks it's writing JSON. Block B
thinks it's reading protobuf. Both are correct—they're exchanging Values.

For Blocks with the same format, the runtime may pass bytes directly without
decode/encode (an optimization, not a guarantee).

### Implications

**Schema is orthogonal to serialization.** A protobuf `.proto` file or OpenAPI
spec defines the *shape* of Values a Block accepts. This is separate from which
encoding carries those Values on the wire.

**Format translation is the runtime's job.** Blocks don't need adapters to talk
to Blocks with different formats. The runtime handles it.

**One format simplifies Blocks.** A Block doesn't need content-type negotiation
or multiple encoders. It picks one format and the runtime does the rest.

**Blocks can wrap existing protocols.** A Block wrapping a gRPC service declares
`application/protobuf` and internally runs standard gRPC handlers. A Block
wrapping an OpenAPI service declares `application/json` and internally handles
REST-style requests. The wrapped service doesn't know it's running on StructFS—
it just sees its native protocol.

This enables middleware that operates at the StructFS Value layer, translating
between protocols:

```
gRPC client → StructFS Value → OpenAPI server
```

The middleware sees Values, agnostic to the serialization on either side.

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

Errors are always expressed in terms of **paths and stores**, never in terms of
implementation details. A caller should not be able to tell whether a path is
served by a simple in-memory store, a remote service, or a complex Assembly of
components. The abstraction must not leak.

### Error Structure

```json
{
    "result": "error",
    "error": {
        "type": "unavailable",
        "message": "Store temporarily unavailable",
        "retryable": true
    }
}
```

The `retryable` field indicates whether the caller should retry the operation:
- `true` — Transient failure, retry may succeed
- `false` — Permanent failure, retry will not help

### Error Categories

#### Path Errors

- **not_found** — No store handles this path (retryable: false)
- **invalid_path** — Path syntax is invalid (retryable: false)

#### Permission Errors

- **not_readable** — Path cannot be read (retryable: false)
- **not_writable** — Path cannot be written (retryable: false)
- **forbidden** — Caller lacks capability (retryable: false)

#### Store Errors

- **unavailable** — Store temporarily unavailable (retryable: true)
- **timeout** — Operation timed out (retryable: true)
- **store_error** — Store-specific error (retryable: depends on details)

#### Value Errors

- **type_mismatch** — Value type doesn't match expectation (retryable: false)
- **validation_failed** — Value failed schema validation (retryable: false)

### Transient vs Permanent Failures

**Transient failures** (retryable: true):
- The store is temporarily overloaded
- A network hiccup occurred
- The store is restarting

**Permanent failures** (retryable: false):
- The path doesn't exist
- The caller lacks permission
- The data is invalid

### Error Transparency

The implementation behind a path should never leak through errors. Compare:

**Wrong** (leaks implementation):
```json
{"type": "block_failed", "message": "Cache block crashed"}
```

**Correct** (store-level abstraction):
```json
{"type": "unavailable", "message": "Store temporarily unavailable", "retryable": true}
```

The caller doesn't know or care that there's a "cache block" — they just know
the store at that path is temporarily unavailable and they can retry.

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
