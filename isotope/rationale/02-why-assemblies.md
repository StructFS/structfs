# Why Assemblies

This document explains the design rationale for Assemblies as Isotope's
composition mechanism.

## The Problem With Flat Process Models

Traditional operating systems have a flat process model:

- Processes are peers
- Any process can (with permission) interact with any other
- Composition is ad-hoc (shell pipelines, container orchestration, service meshes)
- The "shape" of a system is not explicit

This leads to:

1. **Invisible architecture**: You can't look at a running system and see its
   structure. You need diagrams, documentation, tribal knowledge.

2. **Fragile composition**: Connecting services requires configuration (ports,
   hostnames, environment variables) that can drift from reality.

3. **Testing difficulty**: To test a service, you must mock its dependencies.
   But there's no clear boundary for what "its dependencies" means.

4. **Deployment complexity**: Deploying a system of services requires
   orchestration tools (Kubernetes, Docker Compose, systemd) that are separate
   from the services themselves.

## Assemblies as Explicit Composition

An Assembly makes composition explicit and first-class:

```
assembly: my-service
blocks:
  api: ./api-block.wasm
  cache: ./cache-block.wasm
  db: external:postgres-block
wiring:
  api.cache_client -> cache:/
  api.db_client -> db:/queries
exports:
  http: api.http
```

This tells you:
- What the service is made of
- How the parts connect
- What the service exposes

No external documentation required. The Assembly definition *is* the
architecture.

## Assemblies as Blocks

The crucial insight: an Assembly is itself a Block.

This means:
- Assemblies nest to arbitrary depth
- A complex system is built from simpler Assemblies
- Testing an Assembly is the same as testing a Block
- Deployment is recursive: deploy the root Assembly

```
                    ┌─────────────────────────┐
                    │      Application        │
                    │  (root Assembly)        │
                    └───────────┬─────────────┘
                                │
            ┌───────────────────┼───────────────────┐
            │                   │                   │
    ┌───────┴───────┐   ┌───────┴───────┐   ┌───────┴───────┐
    │   Frontend    │   │    Backend    │   │   Infra       │
    │  (Assembly)   │   │  (Assembly)   │   │  (Assembly)   │
    └───────┬───────┘   └───────┬───────┘   └───────┬───────┘
            │                   │                   │
        ┌───┴───┐           ┌───┴───┐           ┌───┴───┐
        │Blocks │           │Blocks │           │Blocks │
        └───────┘           └───────┘           └───────┘
```

Each level sees only its children. Frontend doesn't know Backend exists.
Backend's internal structure is invisible to Frontend.

## Encapsulation

Assemblies provide encapsulation:

1. **Namespace isolation**: A Block in Assembly A cannot see paths in Assembly B

2. **Export control**: Only explicitly exported stores are visible outside

3. **Internal refactoring**: An Assembly's internal structure can change without
   affecting its interface

4. **Clear contracts**: The Assembly definition specifies exactly what goes in
   and what comes out

## Comparison to Alternatives

### Containers (Docker)

Containers provide process isolation but not composition:

- A container is a single process (or init + processes)
- Composition requires external orchestration (Compose, Kubernetes)
- No standard for how containers connect

Assemblies are like containers that know how to compose themselves.

### Kubernetes

Kubernetes provides orchestration but at a different level:

- Services are network endpoints
- Composition is via service discovery and networking
- Heavy-weight (requires a cluster)

Assemblies are simpler: just mount paths. No networking required for in-process
composition.

### Actor Systems (Akka, Erlang/OTP)

Actor systems have similar isolation but different composition:

- Actors send messages to addresses
- Supervision trees provide hierarchy
- But actor addresses are typically global or require lookup

Assemblies use paths, which are hierarchical by nature. The namespace *is*
the composition.

### Microservices

Microservices are services that communicate over the network:

- Each service is independently deployable
- Communication via HTTP, gRPC, etc.
- Service mesh provides cross-cutting concerns

Assemblies can contain microservices, but also in-process Blocks. The
communication model is uniform regardless of location.

## Fractal Benefits

The fractal property (Assemblies are Blocks) gives:

### Testing at any level

```
# Test a single Block
test(my_block, mock_inputs) → outputs

# Test an Assembly the same way
test(my_assembly, mock_inputs) → outputs

# Test the whole system the same way
test(root_assembly, mock_inputs) → outputs
```

### Deployment at any level

```
# Deploy a Block to a runtime
runtime.spawn(block)

# Deploy an Assembly the same way
runtime.spawn(assembly)
```

### Scaling at any level

```
# Scale a Block
replicate(block, 3)

# Scale an Assembly (replicates the whole composition)
replicate(assembly, 3)
```

## Trade-offs

Assemblies have costs:

1. **Boilerplate**: Simple uses require Assembly definitions that feel heavy

2. **Indirection**: Path-based wiring adds a layer vs. direct function calls

3. **Learning curve**: Developers must think in terms of composition, not just
   code

4. **Tooling**: Need tools to visualize, validate, and debug Assembly structures

## Prior Art

- **OTP Applications**: Erlang/OTP's application structure with supervision trees
- **OSGi Bundles**: Java's module system with service registries
- **Docker Compose**: YAML-based service composition
- **Kubernetes**: Pod/Deployment/Service hierarchy
- **Nix**: Declarative composition of packages
- **Dhall**: Programmable configuration for infrastructure
- **Pulumi/CDK**: Infrastructure as code
