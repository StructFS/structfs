# Assemblies

An **Assembly** is a composition of Blocks that itself presents as a Block.

## Definition

An Assembly consists of:

1. **A set of Blocks** (which may themselves be Assemblies)
2. **A Public Block** that serves as the Assembly's external interface
3. **Wiring** that connects Blocks internally

From outside, an Assembly looks exactly like a Block. You cannot tell whether
you're talking to a leaf Block or an Assembly of hundreds of Blocks.

## Immutability

An Assembly definition is an **immutable value**. You don't "modify" a running
Assembly—you create a new Assembly version and transition to it.

This is fundamental:

- An Assembly definition is a complete, self-contained specification
- The definition can be serialized, stored, transmitted, and instantiated anywhere
- "Updating" means creating a new version, not mutating in place
- Rollback is just activating an old version

The running state of an Assembly (which Blocks are running, request counts, etc.)
is separate from its definition. State is ephemeral observation; definition is
immutable truth.

See `08-assembly-management.md` for how Assemblies are deployed and updated.

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

Short form (defaults to `application/json` serialization):

```yaml
blocks:
  api: ./api-block.wasm           # Local Wasm file
  cache: registry/cache-block/1.2  # From a registry
  database: ./database-block.wasm
```

Expanded form with explicit serialization and content hash:

```yaml
blocks:
  api:
    wasm: ./api-block.wasm
    hash: sha256/a1b2c3d4e5f6...
    serialization: application/json

  backend:
    wasm: ./backend-block.wasm
    hash: sha256/f6e5d4c3b2a1...
    serialization: application/protobuf

  metrics:
    wasm: ./metrics-block.wasm
    serialization: application/msgpack
```

The `serialization` field declares the encoding format for all communication
with this Block. It must be set before the Block starts—all messages, including
startup and shutdown, use this format. See `06-protocol.md` for details.

#### Content-Addressed Blocks

The `hash` field content-addresses the Block artifact. This is the canonical
identity—the path is a hint for humans, the hash is the truth.

```yaml
blocks:
  api:
    wasm: ./api-block.wasm
    hash: sha256/a1b2c3d4e5f6...
```

Two Assemblies referencing the same hash reference the same immutable artifact,
regardless of the path. This enables:

- **Deduplication**: Identical Blocks stored once
- **Structural sharing**: New Assembly versions share unchanged Blocks
- **Verification**: The runtime can verify artifacts match their declared hash
- **Caching**: Content-addressed artifacts are safely cacheable forever

When an Assembly is instantiated, the runtime resolves the path and verifies
the hash. If the artifact at the path doesn't match the hash, instantiation
fails.

Block references can point to:
- Local Wasm files
- Registry artifacts (namecode paths like `registry/cache-block/1.2`)
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

## Derived Assemblies

An Assembly can extend another Assembly, inheriting its structure and overriding
specific parts. This enables variants without duplicating the entire definition.

```yaml
assembly: user-service-canary
extends: user-service/versions/2024.01.15

blocks:
  api:
    wasm: ./api-canary.wasm
    hash: sha256/newversion...
    # Overrides the api block; other blocks inherited

config:
  api:
    feature_flags:
      new_algorithm: true
    # Overrides api config; other config inherited
```

Inheritance rules:

- `blocks`: Parent blocks are inherited; child can override by name
- `wiring`: Parent wiring is inherited; child can add or override
- `config`: Deep-merged; child values override parent values at each key
- `public`: Inherited unless explicitly overridden
- `imports`: Merged; child can add new imports
- `failure`: Inherited unless explicitly overridden

This is like prototype inheritance for configurations. The child Assembly is
still a complete, self-contained value once resolved—the `extends` is evaluated
at definition time, not runtime.

### Common Patterns

**Environment variants:**

```yaml
# Base
assembly: user-service
version: 2024.01.15
# ... full definition ...

# Development
assembly: user-service-dev
extends: user-service/versions/2024.01.15
config:
  gateway:
    port: 3000
  database:
    connection: postgres/localhost/dev

# Production
assembly: user-service-prod
extends: user-service/versions/2024.01.15
config:
  gateway:
    port: 8080
    tls:
      cert: /secrets/cert.pem
  database:
    connection: postgres/prod-cluster/main
    pool_size: 50
```

**Canary deployments:**

```yaml
assembly: user-service-canary
extends: user-service/versions/2024.01.15
blocks:
  api:
    wasm: ./api-block.wasm
    hash: sha256/experimental...   # New version under test
```

## Scaling via Composition

Scaling is not a runtime feature—it's expressed through Assembly composition.

To run multiple instances of a Block with load balancing:

```yaml
assembly: gateway-pool

blocks:
  router: stdlib/round-robin-router
  worker-0: ./gateway-block.wasm
  worker-1: ./gateway-block.wasm
  worker-2: ./gateway-block.wasm

public: router

wiring:
  router:/backends/0 -> worker-0
  router:/backends/1 -> worker-1
  router:/backends/2 -> worker-2
```

The router Block (from a standard library) implements load balancing. From
outside, `gateway-pool` is just a Block—callers don't know it's actually
three workers behind a router.

For different routing strategies, use different router Blocks:

- `stdlib/round-robin-router` — Distribute requests evenly
- `stdlib/hash-router` — Route by path hash (for cache affinity)
- `stdlib/least-pending-router` — Route to least-busy worker
- Custom routers for application-specific logic

This keeps the runtime simple. The runtime spawns Blocks and routes by wiring.
Load balancing, sharding, and failover are Blocks you compose, not runtime
features.

## Open Questions

1. **Observability**: Should there be standard paths for Assembly-level
   metrics (Block states, request routing, etc.)?

2. **Instance pooling**: Should there be sugar for "N instances of the same
   Block" to avoid repetitive definitions, or is that a tooling concern?

3. **Autoscaling**: How does dynamic scaling work if Assemblies are immutable?
   Is it a higher-level system that creates new Assembly versions, or something
   else?
