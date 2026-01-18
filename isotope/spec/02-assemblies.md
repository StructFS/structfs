# Assemblies

An **Assembly** is a composition of Blocks that itself presents as a Block.

## Definition

An Assembly:

1. Contains one or more Blocks (which may themselves be Assemblies)
2. Defines how operations on its root store route to constituent Blocks
3. Presents a Block interface to its parent
4. Manages the lifecycle of its constituent Blocks

## The Fractal Property

Assemblies are Blocks. This means:

- An Assembly can be mounted anywhere a Block can
- Assemblies can contain Assemblies to arbitrary depth
- The system itself is the root Assembly

```
System (root Assembly)
├── Frontend Assembly
│   ├── Router Block
│   ├── Auth Block
│   └── UI Block
├── Backend Assembly
│   ├── API Gateway Block
│   ├── Service A Assembly
│   │   ├── Handler Block
│   │   └── Cache Block
│   └── Service B Block
└── Infrastructure Assembly
    ├── Logger Block
    ├── Metrics Block
    └── Tracer Block
```

From Service A Assembly's perspective, it doesn't know whether it's contained
in Backend Assembly or mounted directly at the root. It sees only its root
store.

## Assembly Specification

An Assembly is defined by:

1. **Blocks**: The Blocks it contains (by reference or inline)
2. **Mounts**: How paths in the Assembly's root map to Block namespaces
3. **Wiring**: How Block exports connect to other Blocks' inputs
4. **Entrypoint**: Which Block (if any) handles the Assembly's `run()`

### Blocks

Each Block in an Assembly has a local name:

```
blocks:
  router: <block-ref>
  auth: <block-ref>
  handler: <block-ref>
```

The block-ref may be:
- A reference to an external Block artifact
- An inline Block definition
- A reference to another Assembly (since Assemblies are Blocks)

### Mounts

Mounts define how the Assembly's namespace maps to Block namespaces:

```
mounts:
  # Assembly's /input maps to router's /input
  /input -> router:/input

  # Assembly's /output maps to router's /output
  /output -> router:/output

  # Assembly's /config/auth maps to auth's /config
  /config/auth -> auth:/config
```

### Wiring

Wiring connects Block exports to other Blocks' inputs:

```
wiring:
  # Auth Block's "session" export mounts at Router's /services/auth
  auth.session -> router:/services/auth

  # Router's "requests" export mounts at Handler's /input
  router.requests -> handler:/input

  # Handler's "responses" export mounts at Router's /services/handler
  handler.responses -> router:/services/handler
```

### Entrypoint

The entrypoint determines Assembly behavior:

- **No entrypoint**: Assembly is purely reactive (responds to reads/writes)
- **Single entrypoint**: Named Block's `run()` is called as the Assembly's `run()`
- **Multiple entrypoints**: All named Blocks run concurrently

```
entrypoint: router  # Router Block drives the Assembly
```

Or:

```
entrypoints: [router, background_worker]  # Both run concurrently
```

## Routing

When the Assembly receives a read or write, it must route to the appropriate
Block. Routing is determined by the mount table:

```
Assembly receives: read("/output/response")

Mount table:
  /output -> router:/output

Routes to: router.read("/output/response")
         → which becomes router.read("/response") in router's namespace
```

### Path Rewriting

When routing, the matching mount prefix is stripped and the Block's mount
point is prepended:

```
Assembly path:  /config/auth/timeout
Mount:          /config/auth -> auth:/config
Block path:     /config/timeout
```

### Ambiguity

If multiple mounts could match, the longest prefix wins:

```
Mounts:
  /data -> block_a:/storage
  /data/hot -> block_b:/cache

Path /data/hot/key → routes to block_b (longer match)
Path /data/cold/key → routes to block_a
```

## Lifecycle

Assembly lifecycle and Block lifecycles are related but distinct:

1. **Assembly starts**: Constituent Blocks are not yet running
2. **Entrypoint Blocks start**: `run()` called on entrypoint Blocks
3. **Non-entrypoint Blocks**: Started on first access, or eagerly (configurable)
4. **Assembly stops**: All Blocks are signaled to stop
5. **Blocks drain**: Blocks complete current operations
6. **Assembly complete**: All Blocks have stopped

## Visibility and Capability

A Block can only see:

- Paths explicitly mounted into its namespace
- Exports from other Blocks wired to its namespace

A Block CANNOT:

- Enumerate other Blocks in its Assembly
- Access another Block's store directly (only through wiring)
- Discover the Assembly's structure

This is capability-based security: you can only access what you're given a
path to.

## Nested Addressing

When Assemblies nest, addressing is always relative to the nearest Assembly:

```
Root Assembly contains Backend Assembly contains Service Assembly

From Root's perspective:
  - Backend is at /backend
  - Service is invisible (encapsulated within Backend)

From Backend's perspective:
  - Service is at /services/a
  - Root is invisible (Backend only sees its own root)

From Service's perspective:
  - Only sees its own root
  - Doesn't know Backend or Root exist
```

## Open Questions

1. **Lazy vs Eager Block startup**: Should non-entrypoint Blocks start on
   first access, or when the Assembly starts?

2. **Failure propagation**: If a Block fails, does the Assembly fail? Should
   there be supervision/restart semantics?

3. **Hot reloading**: Can a Block be replaced in a running Assembly? What
   happens to in-flight operations?

4. **Circular wiring**: Is it valid for Block A to export to B and B to export
   to A? If so, how is startup ordering determined?
