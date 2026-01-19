# Blocks

A **Block** is the fundamental unit of execution in Isotope.

## Definition

A Block is:

1. **A pico-process**: Single-threaded, isolated, lightweight
2. **A StructFS client**: It reads and writes paths in its namespace
3. **A StructFS server**: It presents exactly one store to the outside world

## The Two Perspectives

### Inside: Block as StructFS Client

From the Block's perspective, it's a program that reads and writes paths. It
has a namespace—a tree of paths—that it can access. Some of those paths are
provided by the Isotope runtime (`/iso/*`), some are services from other Blocks
(e.g., `/services/*`), some are configuration (`/config/*`).

The Block doesn't know it's "a server." It just runs, reads, writes. Like a
POSIX program that reads from stdin and writes to stdout—it doesn't think of
itself as a pipe component.

### Outside: Block as StructFS Store

From the outside (the runtime, other Blocks, parent Assemblies), a Block is a
StructFS store. You can read and write to it. The Block handles those operations
via the **Server Protocol** (see `07-server-protocol.md`).

## Pico-Process Model

Blocks are pico-processes:

- **Single-threaded**: A Block processes one thing at a time. It can block, but
  it doesn't internally parallelize. Think goroutine or greenlet.

- **Memory-isolated**: Each Block runs in its own Wasm sandbox. It cannot access
  memory from other Blocks.

- **Lightweight**: Thousands of Blocks can run in a single Isotope system.

## The Block's Namespace

Every Block has a namespace—the tree of paths it can access. This namespace is
configured by the containing Assembly.

Standard structure:

```
/
├── iso/                    # Isotope system paths (always present)
│   ├── server/             # Server Protocol interface
│   │   └── requests        # Read incoming Requests here
│   ├── self/               # Block identity and state
│   │   ├── id              # Block's unique identifier
│   │   ├── state           # Current lifecycle state
│   │   └── interface       # Write interface declaration here
│   ├── shutdown/           # Lifecycle control
│   │   ├── requested       # Read to check if shutting down
│   │   └── complete        # Write to signal shutdown complete
│   ├── time/               # Time services
│   │   ├── now             # Current time (ISO 8601)
│   │   └── monotonic       # Monotonic counter
│   ├── random/             # Randomness
│   │   ├── uuid            # Random UUID v4
│   │   └── bytes/{n}       # n random bytes
│   └── log/                # Logging
│       ├── debug
│       ├── info
│       ├── warn
│       └── error
│
├── services/               # Other Blocks, wired by Assembly
│   └── ...
│
├── config/                 # Configuration, provided by Assembly
│   └── ...
│
└── ...                     # Whatever else the Assembly mounts
```

The `/iso/` prefix is reserved for Isotope system services. Everything under
`/iso/` is provided by the runtime. Everything else is wired by the Assembly.

## Block Interface

A Block implements a run loop:

```
loop:
    request = read("/iso/server/requests")
    if request is shutdown signal:
        break
    response = handle(request)
    write(request.respond_to, response)

write("/iso/shutdown/complete", {})
```

The Block:
1. Reads Requests from `/iso/server/requests`
2. Processes each Request
3. Writes Responses to the path specified in each Request
4. Continues until shutdown is signaled

### Batch Processing

Blocks can also read multiple pending requests at once:

```
requests = read("/iso/server/requests/pending")
for request in requests:
    response = handle(request)
    write(request.respond_to, response)
```

This is useful for Blocks that want to batch or prioritize work.

## Self-Description

Blocks describe their interface in two ways:

### Static Declaration

On startup, a Block writes its interface to `/iso/self/interface`:

```
write("/iso/self/interface", {
    "paths": {
        "/": {
            "read": "Returns server status"
        },
        "/log/{level}": {
            "write": "Log a message at the specified level"
        },
        "/log/recent": {
            "read": "Returns recent log entries"
        }
    }
})
```

This allows tooling (Assembly validators, documentation generators) to
understand the Block's interface without running it.

### Runtime Introspection (Meta Lens)

Blocks should respond to `meta/` requests, following the StructFS meta lens
pattern:

```
Request: {op: "read", path: "meta/", ...}
Response: {
    "paths": {
        "/": {"readable": true, "writable": false},
        "/log/{level}": {"readable": false, "writable": true},
        ...
    }
}
```

This allows runtime introspection of a Block's capabilities.

## Identity

Every Block has an identity, readable at `/iso/self/id`. This identity is:

- Unique within the Isotope system
- Assigned by the runtime
- Opaque (the Block shouldn't parse or interpret it)

Block identity is used by the runtime for:

- Routing operations to the correct Block
- Managing Block lifecycle
- Diagnostics and observability

## Isolation Guarantees

A Block MUST NOT be able to:

1. Access memory outside its Wasm sandbox
2. Access paths not in its namespace
3. Discover what other Blocks exist (except through paths wired to it)
4. Bypass StructFS to communicate with anything

A Block MAY:

1. Block on reads (waiting for requests, waiting for responses)
2. Consume CPU and memory (quotas are a runtime concern)

## Block vs Traditional Process

| Aspect | Traditional Process | Isotope Block |
|--------|-------------------|---------------|
| Isolation | Address space | Wasm sandbox + namespace |
| IPC | Pipes, sockets, signals | StructFS read/write |
| Syscalls | Trap to kernel | Read/write `/iso/*` paths |
| Identity | PID (global) | UUID (scoped) |
| Composition | Fork/exec | Assembly wiring |
| Threading | Multi-threaded | Single-threaded |
| Overhead | Heavy | Lightweight (pico-process) |

## Open Questions

1. **Startup handshake**: Should there be a standardized startup sequence where
   the Block declares readiness? Or is "Block started reading requests" enough?

2. **Resource limits**: How does a Block discover its resource limits (memory,
   CPU)? A path under `/iso/limits/*`?

3. **Tracing/debugging**: Should there be standard paths for emitting trace
   spans, or is that an extension?
