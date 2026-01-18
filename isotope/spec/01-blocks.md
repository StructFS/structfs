# Blocks

A **Block** is the fundamental unit of execution in Isotope.

## Definition

A Block is an isolated computational unit that:

1. Has a unique identity within its containing Assembly
2. Sees exactly one store: its **root store**
3. Interacts with the world exclusively through read/write on its root store
4. May export stores for other Blocks to consume

## The Root Store

Every Block has a root store. This is the Block's entire view of the world.
The Block cannot access anything not mounted into this store.

```
Block's view:
/
├── input/          # Data provided to the Block
├── output/         # Data produced by the Block
├── config/         # Block configuration
├── ctx/            # System context (see 04-context.md)
└── services/       # Other Blocks' exports mounted here
```

The specific structure depends on how the Block is mounted into an Assembly.
The above is conventional but not required.

## Block Interface

A Block implements exactly one operation:

```
run() -> Result<(), Error>
```

When a Block is started, `run()` is called. The Block executes until:

- It returns `Ok(())` — normal completion
- It returns `Err(e)` — abnormal completion
- It is terminated by its parent Assembly

During execution, the Block may:

- Read from any path in its root store
- Write to any path in its root store
- Block waiting for data to appear at a path
- Export stores for other Blocks

## Exports

A Block may **export** stores. An export makes a portion of the Block's
functionality available for other Blocks to mount.

```
Block A exports "log" store
    ↓
Assembly mounts A's "log" at B's "/services/logger"
    ↓
Block B writes to "/services/logger/info"
    ↓
Block A's "log" store receives the write
```

Exports are the mechanism for inter-Block services. A Block that provides
logging exports a log store. A Block that provides HTTP exports a request/
response store. And so on.

## Identity

Every Block has an identity within its Assembly. This identity is:

- Unique within the Assembly
- Stable across restarts (if the Assembly specification doesn't change)
- Opaque to the Block itself (a Block cannot discover its own ID)

Block identity is used by the Assembly for:

- Routing operations to the correct Block
- Managing Block lifecycle
- Addressing in diagnostics and observability

## Isolation Guarantees

A Block MUST NOT be able to:

1. Access memory outside its sandbox
2. Access paths not in its root store
3. Discover what other Blocks exist (except through explicit paths)
4. Bypass the store interface to communicate

A Block MAY be able to:

1. Consume unbounded CPU (rate limiting is an Assembly concern)
2. Consume unbounded memory (quotas are an Assembly concern)
3. Block indefinitely on a read (deadlock detection is an Assembly concern)

## Block vs Process

| Aspect | Traditional Process | Isotope Block |
|--------|-------------------|---------------|
| Isolation | Address space | Store namespace |
| IPC | Pipes, sockets, signals | Store reads/writes |
| Syscalls | Trap to kernel | Write to `/ctx/` paths |
| Identity | PID (global) | Path (scoped to Assembly) |
| Composition | Fork/exec | Assembly mounting |
| Overhead | Heavy (pages, descriptors) | Light (just a namespace) |

## Open Questions

1. **Blocking semantics**: When a Block reads a path with no value, does it:
   - Return immediately with "not found"?
   - Block until a value appears?
   - Depend on the path/store?

2. **Cancellation**: How is a Block notified that it should terminate?
   - A special path it polls?
   - An out-of-band signal?
   - Write to a "shutdown" path in its namespace?

3. **Reentrancy**: If Block A writes to Block B which writes back to A in the
   same operation, what happens?
