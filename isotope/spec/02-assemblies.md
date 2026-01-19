# Assemblies

An **Assembly** is a composition of Blocks that itself presents as a Block.

## Definition

An Assembly consists of:

1. **A set of Blocks** (which may themselves be Assemblies)
2. **A Public Block** that serves as the Assembly's external interface
3. **Wiring** that connects Blocks internally

From outside, an Assembly looks exactly like a Block. You cannot tell whether
you're talking to a leaf Block or an Assembly of hundreds of Blocks.

## The Fractal Property

Assemblies are Blocks. This means:

- An Assembly can be used anywhere a Block can
- Assemblies can contain Assemblies to arbitrary depth
- The entire Isotope system is just the outermost Assembly

```
System (root Assembly)
├── Frontend Assembly
│   ├── Router Block (public)
│   ├── Auth Block
│   └── UI Block
├── Backend Assembly
│   ├── API Gateway Block (public)
│   ├── User Service Assembly
│   │   ├── API Block (public)
│   │   ├── Cache Block
│   │   └── DB Block
│   └── Order Service Block
└── Infrastructure Assembly
    ├── Logger Block (public)
    ├── Metrics Block
    └── Tracer Block
```

From Backend's perspective, User Service is just a Block—it doesn't know (or
care) that it's actually an Assembly of three Blocks.

## Assembly Specification

An Assembly is defined by:

```yaml
assembly: user-service
version: 1.0.0

blocks:
  api: ./api-block.wasm
  cache: ./cache-block.wasm
  database: ./database-block.wasm

public: api    # The external face of this Assembly

wiring:
  # api can access cache at /services/cache
  api:/services/cache -> cache

  # api can access database at /services/db
  api:/services/db -> database

  # cache can access database (for cache-aside pattern)
  cache:/services/db -> database

config:
  api:
    timeout: 30s
  cache:
    ttl: 300
  database:
    pool_size: 10
```

### Blocks Section

Lists the Blocks in this Assembly. Each Block has a local name used in wiring.

```yaml
blocks:
  api: ./api-block.wasm           # Local Wasm file
  cache: registry:cache-block:1.2  # From a registry
  database: ./database-block.wasm
```

Block references can point to:
- Local Wasm files
- Registry artifacts
- Other Assembly definitions (since Assemblies are Blocks)

### Public Block

The `public` field names which Block serves as the Assembly's external interface.

```yaml
public: api
```

When something outside reads or writes to this Assembly, those operations go
to the `api` Block. The `api` Block's store IS this Assembly's store.

### Wiring

Wiring connects Blocks within the Assembly. It specifies what paths in one
Block's namespace point to which other Blocks.

```yaml
wiring:
  # Syntax: <block>:<path> -> <target-block>
  api:/services/cache -> cache
  api:/services/db -> database
  cache:/services/db -> database
```

When `api` reads from `/services/cache/hot/key`, the request is routed to the
`cache` Block with path `/hot/key` (the `/services/cache` prefix is stripped).

#### Path Rewriting

Wiring always strips the mount prefix:

```
api writes to:  /services/cache/users/123
cache sees:     /users/123
```

The target Block doesn't know where it's mounted. It sees paths relative to
its own root. This maintains location transparency—a Block's code doesn't
change based on where it's wired.

#### Bidirectional Wiring

Wiring can be bidirectional:

```yaml
wiring:
  api:/services/auth -> auth
  auth:/services/api -> api    # Auth can call back to API
```

This enables patterns like OAuth callbacks, webhooks, and recursive algorithms.
Cycles are allowed—the Server Protocol's async nature prevents deadlock (see
`07-server-protocol.md`).

#### Wiring to Parent Services

An Assembly can expose external services to its internal Blocks:

```yaml
assembly: my-service

# External services this Assembly expects to be wired to
imports:
  logger: "Logging service"
  metrics: "Metrics emission"

blocks:
  api: ./api-block.wasm
  worker: ./worker-block.wasm

public: api

wiring:
  # Internal wiring
  api:/services/worker -> worker

  # Wire external services to internal blocks
  api:/services/logger -> $logger
  worker:/services/logger -> $logger
  worker:/services/metrics -> $metrics
```

The `$logger` and `$metrics` references refer to services that the parent
Assembly must provide when instantiating this Assembly.

## Lifecycle

### Startup

1. Assembly is created (no Blocks running yet)
2. Public Block is started
3. Other Blocks start **lazily** on first access

Lazy startup means a Block doesn't run until something tries to communicate
with it. This enables massive Assemblies where most Blocks are idle.

A Block can "eagerly" start other Blocks by immediately writing to them:

```
// In public Block's startup
write("/services/cache/ping", {})   // Starts cache Block
write("/services/db/ping", {})      // Starts database Block
```

### Shutdown

1. Public Block receives shutdown signal
2. Public Block stops accepting new requests
3. Public Block completes in-flight work
4. Public Block signals shutdown complete
5. Runtime propagates shutdown to other running Blocks
6. All Blocks drain and stop
7. Assembly is stopped

### Failure Handling

When an internal Block fails, the Assembly must decide what to do:

```yaml
assembly: my-service

blocks:
  api: ./api-block.wasm
  cache: ./cache-block.wasm

public: api

failure:
  cache: isolate    # Cache failure doesn't kill the Assembly
  # api failure would kill the Assembly (default: fail-fast)
```

Failure modes:

- **fail-fast** (default): Block failure causes Assembly failure
- **isolate**: Block failure is isolated; its paths return errors
- **restart**: Block is restarted on failure (with configurable limits)

## Visibility and Encapsulation

A Block can only see:

- Paths in `/iso/*` (provided by runtime)
- Paths wired by its Assembly

A Block CANNOT:

- Enumerate other Blocks in its Assembly
- Access a Block it's not wired to
- Discover the Assembly's structure

This is capability-based security: if you don't have a path wired, you don't
have access.

### Encapsulation

An Assembly encapsulates its internal structure. From outside:

- You only see the Public Block's interface
- Internal Blocks are invisible
- Internal wiring is invisible

This means an Assembly can refactor internally (split a Block, merge Blocks,
change wiring) without affecting its public interface.

## Config Injection

Assemblies can inject configuration into Blocks:

```yaml
assembly: my-service

blocks:
  api: ./api-block.wasm

public: api

config:
  api:
    timeout: 30s
    max_connections: 100
```

The `api` Block sees this configuration at `/config/`:

```
read("/config/timeout")         -> "30s"
read("/config/max_connections") -> 100
```

Configuration is injected when the Block starts. It's read-only from the
Block's perspective.

## Example: Web Service Assembly

```yaml
assembly: user-service
version: 1.0.0

blocks:
  gateway: ./gateway-block.wasm
  auth: ./auth-block.wasm
  api: ./api-block.wasm
  cache: ./cache-block.wasm
  database: ./database-block.wasm

public: gateway

imports:
  logger: "Logging service"

wiring:
  # Gateway routes to auth and api
  gateway:/services/auth -> auth
  gateway:/services/api -> api

  # API uses cache and database
  api:/services/cache -> cache
  api:/services/db -> database

  # Cache uses database (cache-aside)
  cache:/services/db -> database

  # Everyone can log
  gateway:/services/logger -> $logger
  auth:/services/logger -> $logger
  api:/services/logger -> $logger

config:
  gateway:
    port: 8080
  cache:
    ttl: 300
  database:
    pool_size: 20

failure:
  cache: isolate    # Cache failure is non-fatal
```

External requests hit `gateway`. `gateway` routes to `auth` for authentication,
then to `api` for business logic. `api` uses `cache` and `database`. All of
this is invisible from outside—they just see the `gateway` Block's store.

## Open Questions

1. **Hot reloading**: Can a Block be replaced in a running Assembly? What
   happens to in-flight operations and state?

2. **Versioning**: How do Assemblies handle version compatibility between
   Blocks? Is there a mechanism for version constraints in wiring?

3. **Observability**: Should there be standard paths for Assembly-level
   metrics (Block states, request routing, etc.)?

4. **Dynamic wiring**: Can wiring change at runtime, or is it fixed at
   Assembly startup?
