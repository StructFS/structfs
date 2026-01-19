# Lifecycle

This document specifies the lifecycle states and transitions for Blocks and
Assemblies.

## Block States

A Block exists in one of six states:

```
    ┌─────────┐     ┌─────────┐     ┌─────────┐
    │ Created │────▶│ Starting│────▶│ Running │
    └─────────┘     └─────────┘     └────┬────┘
                          │              │
                          │         ┌────┴────┐
                          │         │         │
                          ▼         ▼         ▼
                    ┌──────────┐  ┌──────────┐
                    │  Failed  │  │ Stopping │
                    └──────────┘  └────┬─────┘
                          ▲            │
                          │            ▼
                          │      ┌──────────┐
                          └──────│ Stopped  │
                                 └──────────┘
```

### Created

The Block exists but has not been started. It has an identity and namespace
configuration, but no code is running.

**Transitions:**
- → Starting: When the runtime starts the Block (on first access, or at Assembly startup for public Blocks)

### Starting

The Block is initializing. Its run function has been called but it hasn't yet
begun reading requests.

**Transitions:**
- → Running: Block begins reading from `/iso/server/requests`
- → Failed: Error during initialization

### Running

The Block is processing requests. It reads from `/iso/server/requests` and
writes responses.

**Transitions:**
- → Stopping: Shutdown signaled
- → Failed: Unrecoverable error

### Stopping

The Block has been asked to shut down. It should:

1. Stop reading new requests
2. Complete in-flight work
3. Write to `/iso/shutdown/complete`

**Transitions:**
- → Stopped: Block wrote to `/iso/shutdown/complete` and exited
- → Failed: Error during shutdown, or shutdown timeout

### Stopped

The Block has terminated normally. Its store is no longer available.

**Terminal state.**

### Failed

The Block has terminated abnormally. Error information should be available
for diagnostics.

**Terminal state.**

## Lazy Startup

Blocks start **lazily** by default. A Block in Created state transitions to
Starting when:

1. An operation is routed to it (someone reads/writes to its store)
2. It's the public Block of a starting Assembly
3. Another Block explicitly "pokes" it by writing to it

This enables massive Assemblies where most Blocks are idle. Only the Blocks
that receive traffic actually run.

### Eager Startup Pattern

If a Block needs its dependencies running immediately, it can poke them:

```
// In public Block's initialization
write("/services/cache/health", {})   // Starts cache Block
write("/services/db/health", {})      // Starts database Block
```

## Shutdown Protocol

### Normal Shutdown

1. Runtime sets shutdown flag (readable at `/iso/shutdown/requested`)
2. Block's next read from `/iso/server/requests` returns shutdown signal
   (or Block checks `/iso/shutdown/requested` directly)
3. Block stops accepting new work
4. Block completes in-flight operations
5. Block writes to `/iso/shutdown/complete`
6. Block exits its run loop
7. Runtime terminates the Block

### Shutdown Timeout

The runtime may enforce a shutdown timeout. If a Block doesn't reach Stopped
within the timeout:

1. Runtime forcibly terminates the Block
2. Block state becomes Failed
3. In-flight operations may be lost

### Cascading Shutdown (Assemblies)

When an Assembly shuts down:

1. Public Block receives shutdown signal
2. Public Block stops accepting external requests
3. Public Block completes in-flight work
4. Public Block writes to `/iso/shutdown/complete`
5. Runtime propagates shutdown to all running internal Blocks
6. Internal Blocks perform their shutdown sequences
7. Assembly enters Stopped (or Failed if any Block failed)

## Assembly Lifecycle

Assemblies follow the same state machine as Blocks. An Assembly's state reflects
its public Block's state:

| Assembly State | Meaning |
|---------------|---------|
| Created | No Blocks running |
| Starting | Public Block is starting |
| Running | Public Block is running |
| Stopping | Public Block is stopping |
| Stopped | All Blocks stopped |
| Failed | Public Block failed, or shutdown failed |

### Internal Block States

Internal Blocks (non-public) have their own states independent of the Assembly:

- Most start in Created
- Transition to Running when first accessed
- Transition to Stopping when Assembly shuts down
- May be in Failed if they crash

## Failure Handling

When a Block fails, the Assembly decides what to do based on configuration:

### Fail-Fast (Default)

Block failure causes Assembly failure:

```
Block api fails
→ Assembly enters Stopping
→ All Blocks receive shutdown
→ Assembly enters Failed
```

### Isolate

Block failure is isolated:

```yaml
failure:
  cache: isolate
```

```
Block cache fails
→ Operations to cache return errors
→ Assembly remains Running
→ Other Blocks unaffected
```

### Restart

Block failure triggers restart:

```yaml
failure:
  worker:
    policy: restart
    max_restarts: 3
    window: 60s
```

```
Block worker fails
→ Runtime restarts worker (if under limit)
→ New worker instance created
→ Operations resume
```

Restart creates a fresh Block instance. State is not preserved (stateful
restart would require checkpointing, which is out of scope).

## Observability

Block state is observable:

```
read("/iso/self/state") → "running"
```

For diagnostics, runtimes may provide:

```
/iso/self/started_at      # When Block started
/iso/self/request_count   # Requests processed
/iso/self/last_error      # Last error (if any)
```

## State Queries from Outside

Within an Assembly, you generally cannot query another Block's state directly.
If you need to know if a service is healthy:

1. Write to its health endpoint
2. Check the response

This maintains the principle that Blocks only communicate via StructFS
operations, not out-of-band state queries.

## Open Questions

1. **Startup readiness**: Should Blocks signal "ready" explicitly, or is
   "started reading requests" sufficient?

2. **Health checks**: Should there be a standardized health check protocol
   (like Kubernetes readiness/liveness probes)?

3. **Crash recovery**: If a Block crashes and restarts, how do callers know?
   Should there be a "generation" counter?

4. **Preemption**: Can a Running Block be paused and later resumed?

5. **Checkpointing**: Can a Block's state be saved for later restoration
   or migration?
