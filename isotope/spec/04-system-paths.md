# System Paths

The **System Paths** are a set of services mounted at `/iso/` in every Block's
namespace. They are Isotope's equivalent to system calls.

## Design Principle

Traditional operating systems have a system call interface: numbered functions
that trap into the kernel. This interface is fixed at compile time, requires
special CPU instructions, and differs between operating systems.

Isotope has no system calls. Instead, system services are StructFS paths. This
means:

- System services use the same interface as everything else (read/write)
- The runtime can evolve without changing the interface
- Services can be mocked for testing
- The "kernel" is just a store provider

## The `/iso/` Namespace

Every Block has `/iso/` in its namespace. This prefix is reserved—Assemblies
cannot wire paths under `/iso/`.

```
/iso/
├── server/             # Server Protocol (see 07-server-protocol.md)
│   ├── requests        # Read incoming requests
│   └── requests/pending # Batch read pending requests
│
├── self/               # Block identity and state
│   ├── id              # Unique identifier
│   ├── state           # Lifecycle state (created/running/stopping/...)
│   └── interface       # Write interface declaration
│
├── shutdown/           # Lifecycle control
│   ├── requested       # Read: is shutdown requested?
│   └── complete        # Write: signal shutdown complete
│
├── time/               # Time services
│   ├── now             # Current time (ISO 8601)
│   ├── monotonic       # Monotonic counter (for durations)
│   └── zone            # Current timezone
│
├── random/             # Randomness
│   ├── uuid            # Random UUID v4
│   ├── int             # Random integer
│   └── bytes/{n}       # n random bytes
│
└── log/                # Logging
    ├── debug           # Write debug messages
    ├── info            # Write info messages
    ├── warn            # Write warnings
    └── error           # Write errors
```

## Server Protocol Paths

### `/iso/server/requests`

Read to receive the next incoming Request. This is the core of the Server
Protocol—how a Block serves its public store.

```
read("/iso/server/requests") → {
    "op": "read" | "write",
    "path": "users/123",
    "data": {...},          // For writes
    "respond_to": "/iso/server/responses/abc123"
}
```

The read blocks until a request is available.

### `/iso/server/requests/pending`

Read to receive all pending requests at once:

```
read("/iso/server/requests/pending") → [
    {request1},
    {request2},
    ...
]
```

Returns an empty array if no requests are pending (non-blocking).

See `07-server-protocol.md` for complete Server Protocol details.

## Block Identity Paths

### `/iso/self/id`

The Block's unique identifier:

```
read("/iso/self/id") → "block-a1b2c3d4-e5f6-..."
```

This ID is:
- Assigned by the runtime
- Unique within the Isotope system
- Stable for the Block's lifetime
- Opaque (don't parse it)

### `/iso/self/state`

The Block's lifecycle state:

```
read("/iso/self/state") → "running"
```

Values: `created`, `starting`, `running`, `stopping`, `stopped`, `failed`

### `/iso/self/interface`

Write the Block's interface declaration here at startup:

```
write("/iso/self/interface", {
    "name": "cache-service",
    "version": "1.0.0",
    "paths": {
        "/": {"read": "Service status"},
        "/{key}": {
            "read": "Get cached value",
            "write": "Set cached value"
        },
        "/invalidate": {"write": "Invalidate cache"}
    }
})
```

This enables:
- Assembly validation (do wired Blocks have compatible interfaces?)
- Documentation generation
- Tooling and IDE support

## Lifecycle Paths

### `/iso/shutdown/requested`

Check if shutdown has been requested:

```
read("/iso/shutdown/requested") → true | false
```

Blocks should periodically check this (or check after each request) to know
when to begin graceful shutdown.

### `/iso/shutdown/complete`

Signal that shutdown is complete:

```
write("/iso/shutdown/complete", {})
```

After writing this, the Block should exit its run loop. The runtime will
terminate the Block.

## Time Paths

### `/iso/time/now`

Current wall-clock time:

```
read("/iso/time/now") → "2024-01-15T10:30:00Z"
```

ISO 8601 format, UTC.

### `/iso/time/monotonic`

Monotonic counter for measuring durations:

```
start = read("/iso/time/monotonic")
// ... do work ...
end = read("/iso/time/monotonic")
duration = end - start  // Nanoseconds
```

This counter only goes forward, unaffected by wall-clock adjustments.

### `/iso/time/zone`

Current timezone:

```
read("/iso/time/zone") → "America/Los_Angeles"
```

## Random Paths

### `/iso/random/uuid`

Random UUID v4:

```
read("/iso/random/uuid") → "550e8400-e29b-41d4-a716-446655440000"
```

### `/iso/random/int`

Random 64-bit integer:

```
read("/iso/random/int") → 7294619283746192837
```

### `/iso/random/bytes/{n}`

n random bytes:

```
read("/iso/random/bytes/16") → <16 random bytes>
```

## Logging Paths

### `/iso/log/{level}`

Write log messages:

```
write("/iso/log/info", {
    "msg": "Request processed",
    "fields": {
        "request_id": "abc123",
        "duration_ms": 42
    }
})
```

Levels: `debug`, `info`, `warn`, `error`

Writes are fire-and-forget. The runtime handles log routing, filtering, and
aggregation. Blocks don't control where logs go.

## Comparison to Traditional Syscalls

| Traditional | Isotope |
|------------|---------|
| `getpid()` | `read /iso/self/id` |
| `time()` | `read /iso/time/now` |
| `read(fd, ...)` | `read /path/to/resource` |
| `write(fd, ...)` | `write /path/to/resource` |
| `exit(code)` | `write /iso/shutdown/complete` |

## Extension Paths

The `/iso/` namespace can be extended by runtimes. Common extensions might
include:

```
/iso/env/{var}        # Environment variables
/iso/metrics/         # Metrics emission
/iso/trace/           # Distributed tracing
/iso/limits/          # Resource quotas
/iso/debug/           # Debug introspection
```

These are not part of the core specification but demonstrate extensibility.

## Open Questions

1. **Blocking behavior**: When reading `/iso/server/requests` with no pending
   requests, how long does it block? Forever? With timeout?

2. **Shutdown interruption**: How does shutdown interrupt a blocked read on
   `/iso/server/requests`?

3. **Resource limits**: Should there be `/iso/limits/memory`, `/iso/limits/cpu`
   for Blocks to discover their quotas?

4. **Metrics emission**: Should `/iso/metrics/` be standardized for emitting
   metrics, or is that an extension?
