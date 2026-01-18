# Lifecycle

This document specifies the lifecycle states and transitions for Blocks and
Assemblies.

## Block States

A Block exists in one of five states:

```
         ┌──────────────────────────────────────────┐
         │                                          │
         ▼                                          │
    ┌─────────┐     ┌─────────┐     ┌─────────┐    │
    │ Created │────▶│ Starting│────▶│ Running │────┘
    └─────────┘     └─────────┘     └────┬────┘
                                         │
                    ┌────────────────────┼────────────────────┐
                    │                    │                    │
                    ▼                    ▼                    ▼
              ┌──────────┐        ┌──────────┐        ┌──────────┐
              │ Stopping │───────▶│ Stopped  │        │  Failed  │
              └──────────┘        └──────────┘        └──────────┘
```

### Created

The Block exists but has not been started. Its namespace is configured but
`run()` has not been called.

**Transitions:**
- → Starting: When the Assembly starts the Block

### Starting

The Block is initializing. `run()` has been called but the Block has not yet
signaled readiness.

**Transitions:**
- → Running: Block signals readiness or begins processing
- → Failed: Initialization error

### Running

The Block is executing normally. It can read and write paths in its namespace.

**Transitions:**
- → Stopping: Shutdown requested (by Assembly or self)
- → Failed: Unrecoverable error

### Stopping

The Block has been asked to shut down and is draining. It should:

1. Stop accepting new work
2. Complete in-flight operations
3. Clean up resources
4. Signal completion

**Transitions:**
- → Stopped: Graceful shutdown complete
- → Failed: Error during shutdown or timeout

### Stopped

The Block has terminated normally. Its exports are no longer available.

**Terminal state.**

### Failed

The Block has terminated abnormally. Error information is available at
`/ctx/iso/proc/{id}/error`.

**Terminal state.**

## State Queries

A Block can query its own state:

```
read /ctx/iso/self/state → "running" | "stopping" | ...
```

A Block can observe the shutdown signal:

```
read /ctx/iso/shutdown/requested → true | false
```

## Shutdown Protocol

When a Block needs to shut down:

1. **Signal**: Assembly writes to Block's shutdown path, or Block observes
   system-wide shutdown
2. **Acknowledge**: Block reads `/ctx/iso/shutdown/requested` and sees `true`
3. **Drain**: Block completes in-flight work, stops accepting new work
4. **Complete**: Block writes to `/ctx/iso/shutdown/complete`
5. **Terminate**: Block's `run()` returns

### Timeouts

The Assembly may enforce a shutdown timeout. If a Block doesn't complete
shutdown within the timeout:

1. Block is forcibly terminated
2. Block state becomes Failed
3. Error indicates timeout

### Cascading Shutdown

When an Assembly shuts down:

1. Assembly enters Stopping state
2. Assembly signals shutdown to all constituent Blocks
3. Blocks begin their shutdown sequences
4. Assembly waits for all Blocks to reach Stopped/Failed
5. Assembly enters Stopped state

## Assembly Lifecycle

Assemblies follow the same state machine as Blocks, with additional semantics
for managing constituent Blocks:

### Assembly Starting

1. Assembly enters Starting state
2. Assembly starts entrypoint Blocks (if any)
3. Non-entrypoint Blocks may start lazily or eagerly (configurable)
4. Assembly enters Running when entrypoints are Running

### Assembly Running

- Entrypoint Blocks are Running
- Other Blocks start on first access (if lazy) or are already Running (if eager)
- Assembly routes operations to constituent Blocks

### Assembly Stopping

1. Assembly enters Stopping state
2. Stop accepting new operations (return "stopping" error)
3. Signal shutdown to all Running Blocks
4. Wait for all Blocks to reach terminal state
5. Assembly enters Stopped (if all Blocks Stopped) or Failed (if any Failed)

## Block Failure Handling

When a Block fails, the Assembly must decide what to do:

### Fail-Fast (Default)

Block failure causes Assembly failure. The Assembly enters Stopping state,
shuts down other Blocks, and fails.

### Isolate

Block failure is isolated. The failing Block's paths return errors, but other
Blocks continue. The Assembly remains Running.

### Restart

Block failure triggers restart. The Assembly creates a new instance of the
Block with fresh state. (Stateful restart requires additional mechanisms.)

### Supervision

More complex policies can be built:

```
supervision:
  critical:        # These Blocks failing causes Assembly failure
    - router
    - auth
  restartable:     # These Blocks are restarted on failure
    - cache
    max_restarts: 3
    window: 60s
  optional:        # These Blocks failing is logged but ignored
    - metrics
```

## Observability

Block and Assembly lifecycle events should be observable:

```
# Watch for state changes
watch /ctx/iso/proc/{id}/state

# Get lifecycle history
read /ctx/iso/proc/{id}/history
→ [
    {"state": "created", "time": "..."},
    {"state": "starting", "time": "..."},
    {"state": "running", "time": "..."},
    ...
  ]
```

## Open Questions

1. **Health checks**: Should there be a standard health check mechanism
   (readiness probe, liveness probe)?

2. **Restart semantics**: When a Block is restarted, does it keep its ID?
   What about its namespace configuration?

3. **Preemption**: Can a Running Block be preempted (paused) and later resumed?

4. **Checkpointing**: Can a Block's state be checkpointed for later restoration
   or migration?
