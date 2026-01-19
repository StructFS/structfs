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

Isotope provides two shutdown modes, analogous to SIGTERM and SIGKILL in POSIX:

### Graceful Shutdown (SIGTERM-like)

Graceful shutdown gives Blocks time to clean up and complete in-flight work.

1. Runtime sets `/iso/shutdown/requested` to `true`
2. Runtime sets `/iso/shutdown/mode` to `"graceful"`
3. Block's next read from `/iso/server/requests` returns `null` (unblocks)
4. Block checks `/iso/shutdown/requested`, sees shutdown in progress
5. Block stops accepting new work
6. Block completes in-flight operations
7. Block performs cleanup (close connections, flush buffers, etc.)
8. Block writes to `/iso/shutdown/complete`
9. Block exits its run loop
10. Runtime terminates the Block, state becomes Stopped

Blocks should handle graceful shutdown cooperatively:

```python
while True:
    request = read("/iso/server/requests")

    if request is None:
        # Shutdown signaled - check mode
        mode = read("/iso/shutdown/mode")
        if mode == "graceful":
            cleanup()
        break

    process(request)

write("/iso/shutdown/complete", {})
```

### Immediate Shutdown (SIGKILL-like)

Immediate shutdown terminates the Block without waiting for cleanup.

1. Runtime sets `/iso/shutdown/mode` to `"immediate"`
2. Runtime forcibly terminates the Block
3. Block code stops executing immediately
4. Block state becomes Stopped (not Failed—this is intentional termination)

The Block has no opportunity to clean up. Use immediate shutdown when:
- Graceful shutdown has timed out
- The Block is unresponsive
- Emergency shutdown is required

### Graceful Timeout to Immediate

The typical pattern is graceful shutdown with a timeout fallback:

1. Request graceful shutdown
2. Wait for timeout (configurable per Block/Assembly)
3. If Block hasn't reached Stopped, escalate to immediate

```yaml
shutdown:
  timeout: 30s          # Wait this long for graceful
  escalate: immediate   # Then force-kill
```

### In-Flight Request Handling

When a Block is shutdown while requests are pending or being processed:

**Graceful shutdown:**
- Block should complete in-flight requests before exiting
- New requests are rejected (runtime returns error to callers)
- The `respond_to` paths remain valid until responses are written

**Immediate shutdown:**
- In-flight requests are NOT completed
- The runtime is responsible for resolving pending requests:
  - Return error responses to waiting callers
  - If restart policy is configured, may spin up new instance and retry
  - Caller sees a store-level error (abstraction does not leak):
    ```json
    {"result": "error", "error": {"type": "unavailable", "retryable": true}}
    ```

### Cascading Shutdown (Assemblies)

When an Assembly receives shutdown:

1. Assembly determines shutdown mode (graceful or immediate)
2. **Graceful path:**
   - Public Block receives graceful shutdown
   - Public Block stops accepting external requests
   - Public Block completes in-flight work and writes `/iso/shutdown/complete`
   - Runtime propagates graceful shutdown to all running internal Blocks
   - Internal Blocks perform their shutdown sequences (recursively for nested Assemblies)
   - Assembly enters Stopped when all Blocks are Stopped
3. **Immediate path:**
   - All Blocks terminated immediately
   - No cleanup opportunity
   - Assembly enters Stopped

### Shutdown Paths

| Path | Read | Write | Description |
|------|------|-------|-------------|
| `/iso/shutdown/requested` | bool | - | True if shutdown has been requested |
| `/iso/shutdown/mode` | string | - | `"graceful"` or `"immediate"` |
| `/iso/shutdown/complete` | - | {} | Block writes here to signal clean exit |

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
