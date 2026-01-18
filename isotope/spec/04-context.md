# Context Root

The **Context Root** is a set of system services mounted at a well-known path
in every Block's namespace. It is Isotope's equivalent to system calls.

## Location

By convention, the context root is mounted at `/ctx/iso`. An Assembly may mount
it elsewhere or restrict access to portions of it, but `/ctx/iso` is the
standard location.

## Design Principle

Traditional operating systems have a system call interface: a set of numbered
functions that trap into the kernel. This interface is:

- Fixed at compile time
- Requires special CPU instructions
- Differs between operating systems

Isotope has no system calls. Instead, system services are stores mounted at
paths. This means:

- System services are just stores (same interface as everything else)
- New services can be added by mounting new stores
- Services can be mocked for testing
- The "kernel" is just another Assembly

## Standard Services

### `/ctx/iso/self` — Block Identity

Information about the current Block:

```
read /ctx/iso/self/id        → Block's ID (opaque string)
read /ctx/iso/self/assembly  → Containing Assembly's ID
read /ctx/iso/self/started   → Timestamp when Block started
read /ctx/iso/self/state     → Current state (running, stopping, etc.)
```

### `/ctx/iso/time` — Time Services

```
read /ctx/iso/time/now       → Current time (ISO 8601)
read /ctx/iso/time/monotonic → Monotonic counter (for measuring durations)
read /ctx/iso/time/zone      → Current timezone
```

### `/ctx/iso/random` — Randomness

```
read /ctx/iso/random/bytes/16   → 16 random bytes
read /ctx/iso/random/uuid       → Random UUID v4
read /ctx/iso/random/int        → Random integer
```

### `/ctx/iso/log` — Logging

```
write /ctx/iso/log/debug {"msg": "...", "fields": {...}}
write /ctx/iso/log/info  {"msg": "...", "fields": {...}}
write /ctx/iso/log/warn  {"msg": "...", "fields": {...}}
write /ctx/iso/log/error {"msg": "...", "fields": {...}}
```

Writes to log paths are fire-and-forget. The log store may buffer, filter,
or route logs based on runtime configuration.

### `/ctx/iso/ns` — Namespace Manipulation

```
read  /ctx/iso/ns/mounts         → List of current mounts
write /ctx/iso/ns/mount          → Add a mount
write /ctx/iso/ns/unmount        → Remove a mount
read  /ctx/iso/ns/resolve/path   → Resolve path to backing store info
```

### `/ctx/iso/proc` — Process (Block) Information

Information about running Blocks (subject to visibility rules):

```
read /ctx/iso/proc/list              → IDs of visible Blocks
read /ctx/iso/proc/{id}/state        → Block's state
read /ctx/iso/proc/{id}/started      → Start timestamp
read /ctx/iso/proc/{id}/exports      → List of exports
```

A Block can only see Blocks that its Assembly has made visible to it.

### `/ctx/iso/ipc` — Inter-Process Communication

Channels for Block-to-Block communication:

```
# Create a channel
write /ctx/iso/ipc/channel/create {"name": "events"}
→ returns channel path

# Send to channel
write /ctx/iso/ipc/channel/{id}/send {"event": "click", ...}

# Receive from channel
read /ctx/iso/ipc/channel/{id}/recv
→ blocks until message available, returns message

# Non-blocking receive
read /ctx/iso/ipc/channel/{id}/try_recv
→ returns message or null immediately
```

### `/ctx/iso/spawn` — Block Spawning

Create new Blocks (if permitted):

```
write /ctx/iso/spawn {
  "block": "my-block-artifact",
  "mounts": {
    "/input": "channel://events",
    "/output": "channel://results"
  }
}
→ returns new Block's ID and handle paths
```

### `/ctx/iso/shutdown` — Lifecycle Control

```
write /ctx/iso/shutdown/request {}      → Request graceful shutdown
read  /ctx/iso/shutdown/requested       → Check if shutdown requested
write /ctx/iso/shutdown/complete {}     → Signal shutdown complete
```

## Capability Restriction

An Assembly can restrict which context services a Block can access by not
mounting the full context root:

```
# Block only gets time and logging, no spawning or IPC
mounts:
  /ctx/iso/time → system:time
  /ctx/iso/log  → system:log
  # /ctx/iso/spawn not mounted — Block cannot spawn
  # /ctx/iso/ipc not mounted — Block cannot use IPC
```

This is capability-based security: if you don't have the path, you don't have
the capability.

## Extension Services

The context root is extensible. Additional services can be mounted:

```
/ctx/iso/http     → HTTP client (like StructFS's HTTP broker)
/ctx/iso/fs       → File system access (if permitted)
/ctx/iso/env      → Environment variables
/ctx/iso/metrics  → Metrics emission
/ctx/iso/trace    → Distributed tracing
```

These are not part of the core specification but demonstrate the extensibility
of the context root model.

## Comparison to Traditional Syscalls

| Traditional | Isotope |
|------------|---------|
| `getpid()` | `read /ctx/iso/self/id` |
| `time()` | `read /ctx/iso/time/now` |
| `read(fd, ...)` | `read /path/to/resource` |
| `write(fd, ...)` | `write /path/to/resource` |
| `fork()` | `write /ctx/iso/spawn {...}` |
| `exit(code)` | `write /ctx/iso/shutdown/complete {"code": n}` |
| `kill(pid, sig)` | `write /ctx/iso/proc/{id}/signal {"signal": "..."}` |

## Open Questions

1. **Blocking reads**: Should `/ctx/iso/ipc/channel/{id}/recv` block forever,
   or should there be a timeout? How does a Block get interrupted?

2. **Permissions model**: How are context capabilities granted/revoked? Just
   mount/unmount, or something more granular?

3. **Observability**: Should there be standard paths for emitting metrics
   and traces, or is that an extension concern?

4. **Resource limits**: Should there be `/ctx/iso/limits` for querying/setting
   resource quotas (memory, CPU, etc.)?
