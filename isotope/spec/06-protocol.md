# Protocol

This document specifies the semantics of store operations in Isotope. These
operations form the "protocol" through which all system interaction occurs.

## Operations

There are two fundamental operations:

### Read

```
read(path) → Result<Option<Record>, Error>
```

Read the value at `path`. Returns:

- `Ok(Some(record))` — Value exists
- `Ok(None)` — Path exists but has no value (or value is explicitly null)
- `Err(error)` — Operation failed

### Write

```
write(path, record) → Result<Path, Error>
```

Write `record` to `path`. Returns:

- `Ok(result_path)` — Write succeeded, returns the path written to
- `Err(error)` — Operation failed

The `result_path` may differ from `path` in append-style stores:

```
write /log/entries {"msg": "hello"}
→ Ok(/log/entries/0)  # Actual path where entry was stored
```

## Records

A Record is the unit of data exchange. It contains:

- **Value**: The actual data (see below)
- **Format**: Hint about how to interpret the bytes (optional)

### Value Types

Values follow StructFS's Value type:

- Null
- Bool
- Integer (signed 64-bit)
- Float (64-bit)
- String (UTF-8)
- Bytes (arbitrary octets)
- Array (ordered list of Values)
- Map (string-keyed collection of Values)

## Blocking Semantics

### Non-Blocking (Default)

By default, operations are non-blocking:

- Read returns immediately with current state
- Write returns immediately after accepting the write

### Blocking Reads

Some stores support blocking reads that wait for data:

```
read /channel/events  # Blocks until an event is available
```

Blocking is a property of the store, not the operation. A Block cannot force
a non-blocking store to block.

### Blocking Writes

Some stores may block writes:

```
write /queue/jobs {...}  # Blocks if queue is full
```

Again, this is store-dependent.

## Consistency

Isotope does not mandate a global consistency model. Each store defines its
own consistency guarantees:

### Strong Consistency

Read always returns the result of the most recent write:

```
write /data/x {"v": 1}
read /data/x  → {"v": 1}  # Guaranteed
```

### Eventual Consistency

Read may return stale data:

```
write /cache/x {"v": 1}
read /cache/x  → {"v": 0}  # Might still see old value
# ... eventually ...
read /cache/x  → {"v": 1}  # Eventually consistent
```

### Causal Consistency

Operations from the same Block are seen in order:

```
# Block A:
write /shared/x {"v": 1}
write /shared/y {"v": 2}

# Block B observes:
# If it sees y=2, it must see x=1
# But it might see x=1 without y=2
```

## Error Types

Errors fall into categories:

### Path Errors

- `PathNotFound` — No store handles this path
- `InvalidPath` — Path syntax is invalid

### Permission Errors

- `NotReadable` — Path exists but cannot be read
- `NotWritable` — Path exists but cannot be written
- `Forbidden` — Caller lacks capability for this operation

### Store Errors

- `StoreError(details)` — Store-specific error
- `Timeout` — Operation timed out
- `Unavailable` — Store temporarily unavailable

### Value Errors

- `TypeMismatch` — Value type doesn't match expectation
- `ValidationFailed` — Value failed schema validation

## Concurrency

Multiple Blocks may read and write the same paths concurrently. The store
determines how concurrent operations interact.

### No Guarantees

Concurrent writes may interleave arbitrarily:

```
# Block A: write /data {"a": 1}
# Block B: write /data {"b": 2}
# Result: {"a": 1} or {"b": 2} or (if store merges) {"a": 1, "b": 2}
```

### Last-Writer-Wins

Most recent write overwrites previous:

```
# t=1: Block A writes {"a": 1}
# t=2: Block B writes {"b": 2}
# Result: {"b": 2}
```

### Merge

Store merges concurrent writes:

```
# Block A: write /counter/increment {"by": 1}
# Block B: write /counter/increment {"by": 2}
# Result: counter increased by 3 (order doesn't matter)
```

### Transactions

Some stores may support transactions (read-modify-write atomically):

```
write /ctx/iso/tx/begin {}
→ {"tx": "abc123"}

read /data/account {"tx": "abc123"}
→ {"balance": 100}

write /data/account {"balance": 90, "tx": "abc123"}

write /ctx/iso/tx/commit {"tx": "abc123"}
→ Ok if no conflict, Err if concurrent modification
```

This is an extension, not a core requirement.

## Streaming

Some operations may return or accept streams:

### Streaming Reads

```
read /logs/stream
→ Stream of log records, potentially infinite
```

The Block consumes records from the stream until:
- It's read enough
- The stream ends
- An error occurs

### Streaming Writes

```
write /upload/file <stream of chunks>
```

The Block provides chunks until the upload is complete.

Streaming semantics are an extension. The core protocol is request-response.

## Observation (Watch)

Stores may support watching for changes:

```
watch(path) → Stream<Event>
```

Events include:
- `Created(path, value)` — New value at path
- `Updated(path, old, new)` — Value changed
- `Deleted(path, old)` — Value removed

Not all stores support watching. A store that doesn't support watch returns
an error.

## Request/Response Framing

When store operations cross process or network boundaries, they must be
serialized. Isotope does not mandate a wire format, but a request/response
framing might look like:

```
Request:
  id: unique request identifier
  operation: "read" | "write"
  path: string
  record: (for write) serialized Record

Response:
  id: matching request identifier
  result: "ok" | "error"
  record: (for successful read) serialized Record
  path: (for successful write) result path
  error: (for error) error details
```

The specific serialization (JSON, MessagePack, Protobuf, etc.) is an
implementation concern.

## Open Questions

1. **Partial reads**: Should there be a way to read only part of a large
   value (like HTTP Range requests)?

2. **Conditional operations**: Should there be compare-and-swap or
   if-not-modified semantics?

3. **Bulk operations**: Should there be a way to read/write multiple paths
   atomically?

4. **Metadata**: Should operations support metadata (headers) separate from
   the value, like HTTP?
