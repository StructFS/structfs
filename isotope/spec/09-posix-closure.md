# POSIX Closure

This document specifies the additions that close `/iso/` over the needs of
a POSIX-style process, so that code written against a process model — up to
and including code compiled against libc via a WASI shim — can run as a
Block with every "syscall" expressed as a StructFS operation.

The guiding split: **ambient** services (identity, time, randomness,
logging, stdio, events, process control) live under `/iso/` and are always
present; **granted** capabilities (filesystem access, network access) are
NOT ambient — they arrive by Assembly wiring, exactly like any other
service. A WASI preopen is a namespace mount. This keeps the capability
model uniform: `/iso/` is what every process deserves; everything else is
what its Assembly granted.

## The Unified Mailbox

POSIX processes multiplex: `poll(2)` waits on file descriptors, signals,
and timers at once. Isotope Blocks get one blocking read. Rather than
adding a `select` primitive, Isotope adopts the actor answer:
**`/iso/server/requests` is the Block's single event mailbox.** Everything
asynchronous arrives there as an event, interleaved with served
operations:

```json
{"op": "read",   "path": "users/123", "data": null, "respond_to": "..."}
{"op": "write",  "path": "users/123", "data": {...}, "respond_to": "..."}
{"op": "signal", "signal": "usr1", "data": {...}}
{"op": "timer",  "tag": "flush"}
```

- `read`/`write` events are Server Protocol requests (spec 07) and carry
  `respond_to`.
- `signal` events are runtime- or host-originated notifications. They
  have no `respond_to`; delivery is fire-and-forget.
- `timer` events are deliveries for timers the Block registered (below).

A Block that only serves requests ignores events with other ops. A Block
that needs `poll` semantics has them for free: one blocking read yields
whatever happens next. `requests/pending` returns all queued events
without blocking.

Shutdown remains the Null unblock (spec 07): a parked mailbox read
returns Null when shutdown is requested.

## Path Additions

### Identity and environment

```
/iso/self/args          # Array of argument strings (from the Assembly)
/iso/env                # Map of environment variables (from the Assembly)
/iso/env/{name}         # One variable
```

`args` and `env` are declared per-Block in the Assembly definition and
are read-only, like `/config`. They exist because the process model
expects them; new Blocks should prefer `/config`.

### Standard streams

```
/iso/stdio/stdin        # Blocking read: next line (string); null at EOF
/iso/stdio/stdout       # Write: append output
/iso/stdio/stderr       # Write: append error output
```

Stdio is line/value-oriented at this layer (a byte-stream refinement is a
runtime extension). Which Blocks get real stdio is a runtime/Assembly
decision; unattached Blocks read EOF and their writes go to the log.
This closes the "terminal I/O is out-of-band" hole: an interactive Block
does all its I/O through its namespace.

### Time and timers

```
/iso/time/now_unix_ns   # Integer nanoseconds since the Unix epoch
/iso/time/after/{ms}    # Blocking read: parks for {ms} milliseconds
/iso/timers             # Write {"ms": N, "tag": "..."} -> timers/{id};
                        #   a {"op":"timer","tag":...} event is delivered
                        #   to the mailbox at expiry. Write Null to the
                        #   returned path to cancel.
```

`time/after` is synchronous sleep; `timers` is asynchronous wakeup via
the mailbox. Both are cancellable by shutdown.

### Process control

```
/iso/proc               # Write an Assembly definition (as a Value), or
                        #   {"definition": ..., "grants": {...}}
                        #   -> proc/outstanding/{id}   (spawn)
/iso/proc/outstanding/{id}        # Read: {name, state, code?}   (status)
/iso/proc/outstanding/{id}/wait   # Blocking read: parks until terminal,
                                  #   returns {state, code?}     (wait)
/iso/proc/outstanding/{id}/store/... # Read/write: the child's public
                                  #   store (the parent's channel to it)
/iso/proc/outstanding/{id}        # Write Null: shut down and release (kill)
```

Spawn/wait/kill are the handle pattern: `wait(2)` is a blocking read of
a handle, `kill(2)` is the handle release, and a released or abandoned
handle store shuts its children down (no orphans). `/iso/proc` is
present only for Blocks whose definition grants it (`spawn: true`) — the
ability to create processes is a capability like any other.

#### Grants

A spawner may bind the child's declared imports to slices of its own
namespace:

```json
{
    "definition": { ...an Assembly definition with imports... },
    "grants": { "logger": "services/logs" }
}
```

Each grant path must resolve inside the spawner's wired namespace —
**a Block can only delegate capabilities it holds**. The Assembly stays
the root of authority: `spawn: true` is granted by the Assembly, and
spawn-time grants can only attenuate the spawner's own capabilities,
never widen them. The child sees each grant as an import store rooted at
the granted path; result paths never reveal anything above the grant.
Without grants, a spawned Assembly receives only `/iso/` and its own
definition's wiring.

The same protocol, served by the runtime at its management surface,
is how Assemblies are deployed at the top level (spec 08): the system
is managed by writing to a store, not by a separate API.

### Exit codes

`write /iso/shutdown/complete {"code": N}` records an exit code
(default 0). A Block's terminal status — observable via `proc` handles
and management reads — is `{state: "stopped" | "failed", code}`.

## Unwired Paths Are Denied

Spec 03 previously distinguished unwired reads (null) from unwired
writes (error). A capability system should not leak the difference
between "nothing is there" and "you may not know": **both reads and
writes to unwired paths fail with `forbidden`.** Absence (`null`) is a
statement a wired store makes about its own contents, not something the
namespace invents.

## Error Taxonomy and errno

The protocol error types (spec 06) map onto errno for shim layers:

| Protocol error | errno |
|---|---|
| `not_found` | `ENOENT` |
| `forbidden`, `not_readable`, `not_writable` | `EACCES` |
| `unavailable` | `EAGAIN` |
| `timeout` | `ETIMEDOUT` |
| `conflict` | `EEXIST` |
| cancellation (a released handle / interrupted parked read) | `EINTR` |
| `invalid_path` | `EINVAL` |

Cancellation-fails-reads is exactly the interrupted-syscall semantic.

## WASI Correspondence

A WASI shim (implemented separately) maps host calls onto this surface:

| WASI | Isotope |
|---|---|
| `args_get`, `environ_get` | `/iso/self/args`, `/iso/env` |
| `clock_time_get` | `/iso/time/now_unix_ns` |
| `random_get` | `/iso/random/bytes/{n}` |
| `fd_read(0)`, `fd_write(1|2)` | `/iso/stdio/*` |
| `proc_exit(code)` | `/iso/shutdown/complete {"code": N}` |
| `poll_oneoff` | the unified mailbox |
| preopens, `path_open`, `fd_*` | **wired** filesystem stores (mounts are preopens) |
| `sock_*` | **wired** network stores |

## Deliberate Rejections

- **No fork.** Spawn is the primitive; there is no address-space cloning.
- **No threads.** Concurrency is composition (spec 02); a Block is
  single-threaded by definition.
- **No shared memory.** Values move by message; stores hold state.
- **No users/groups.** The namespace is the credential; caller identity
  (spec 07 open question 1) remains open but will be capability-shaped,
  not uid-shaped.

## Blocking Paths Are Declared

`read /iso/meta` returns per-path descriptors
`{readable, writable, blocking}`. The `blocking` attribute names the
paths whose reads may legitimately park (`server/requests`,
`stdio/stdin`, `time/after/{ms}`, proc waits), so callers can set
per-operation deadlines instead of one oversized global timeout — a
wedged store should not be indistinguishable from a parked one. See
`docs/patterns/meta.md`.

## Byte Streams

Resolved by the byte-stream pattern (`docs/patterns/bytestream.md`):
ranged reads at `at/{offset}/len/{n}` (matching `structfs-sys` file
handles) with `read(2)` semantics — a parked read returns at least one
byte or an empty result exactly at end-of-stream, offsets are
caller-held cursors, and stale offsets clamp. This is the convention an
`fd_read`/`fd_write` shim maps onto.

## Open Questions

1. **Stdio as byte streams**: line orientation is a strawman; terminals
   want raw mode eventually. The byte-stream pattern is the natural
   refinement target.
2. **Assembly-level delegation control**: whether a Block's definition
   should restrict *which* of its capabilities may be re-granted
   (`spawn: {grantable: [...]}`), beyond the attenuation-only rule.
