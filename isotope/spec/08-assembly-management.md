# Assembly Management

This document specifies how Assemblies are deployed, versioned, and updated.
All management operations happen through StructFS paths—there is no separate
management API.

## Principle: Assemblies as Values

An Assembly definition is a **value**. It can be:

- Serialized to YAML, JSON, or any StructFS-compatible format
- Stored in a file, database, or registry
- Transmitted over the network
- Written to a StructFS path to instantiate it
- Read back from the runtime in identical form

The management API is just StructFS operations on Assembly values. There's no
special management protocol—if you can read and write paths, you can manage
Assemblies.

## The `/iso/assemblies/` Namespace

Assembly management happens under `/iso/assemblies/`:

```
/iso/assemblies/
├── <name>/
│   ├── versions/
│   │   ├── <version>/
│   │   │   ├── definition      # The immutable Assembly definition
│   │   │   └── state           # Runtime state (if instantiated)
│   │   └── ...
│   ├── active                  # Currently active version
│   └── history                 # Activation history
└── ...
```

## Deploying an Assembly

Write the Assembly definition to create a new version:

```
write /iso/assemblies/user-service/versions/2024.01.15/definition {
  "assembly": "user-service",
  "version": "2024.01.15",
  "blocks": {
    "gateway": {"wasm": "./gateway.wasm", "hash": "sha256/abc123..."},
    "api": {"wasm": "./api.wasm", "hash": "sha256/def456..."}
  },
  "public": "gateway",
  "wiring": {
    "gateway:/services/api": "api"
  }
}
```

This stores the definition. It does not instantiate or activate the Assembly.

## Reading an Assembly Definition

```
read /iso/assemblies/user-service/versions/2024.01.15/definition
→ {
    "assembly": "user-service",
    "version": "2024.01.15",
    "blocks": { ... },
    ...
  }
```

The returned value is identical to what was written. Round-trip fidelity is
guaranteed—you can read a definition and write it elsewhere to create a copy.

## Activating a Version

Activation tells the runtime to instantiate and route traffic to a version:

```
write /iso/assemblies/user-service/active {
  "version": "2024.01.15"
}
```

The runtime:

1. Instantiates the Assembly (starts the public Block)
2. Begins routing traffic to the new version
3. Records the activation in history

### Activation Strategies

The `strategy` field controls how traffic transitions:

```
write /iso/assemblies/user-service/active {
  "version": "2024.01.15",
  "strategy": "blue-green"
}
```

Strategies:

- **immediate** (default): Switch all traffic instantly
- **blue-green**: Start new version, verify health, then switch
- **rolling**: Gradually shift traffic over time
- **canary**: Route percentage of traffic to new version

```
write /iso/assemblies/user-service/active {
  "version": "2024.01.15",
  "strategy": "canary",
  "canary": {
    "percent": 10,
    "duration": "1h"
  }
}
```

## Checking Active Version

```
read /iso/assemblies/user-service/active
→ {
    "version": "2024.01.15",
    "since": "2024-01-15T10:30:00Z",
    "strategy": "blue-green",
    "status": "active"
  }
```

## Observing Runtime State

The definition is immutable; the state is ephemeral observation:

```
read /iso/assemblies/user-service/versions/2024.01.15/state
→ {
    "status": "running",
    "started": "2024-01-15T10:30:00Z",
    "blocks": {
      "gateway": {
        "status": "running",
        "instances": 1,
        "pending_requests": 5
      },
      "api": {
        "status": "running",
        "instances": 1,
        "pending_requests": 12
      }
    }
  }
```

State paths only exist for instantiated versions. Reading state for a version
that isn't running returns null.

## Version History

```
read /iso/assemblies/user-service/history
→ {
    "activations": [
      {
        "version": "2024.01.15",
        "activated": "2024-01-15T10:30:00Z",
        "by": "deploy-pipeline"
      },
      {
        "version": "2024.01.14",
        "activated": "2024-01-14T09:00:00Z",
        "deactivated": "2024-01-15T10:30:00Z",
        "by": "deploy-pipeline"
      }
    ]
  }
```

## Rollback

Rollback is just activating an old version:

```
write /iso/assemblies/user-service/active {
  "version": "2024.01.14"
}
```

No special rollback machinery. The old version's definition is still there
(definitions are immutable), so you instantiate it again.

## Listing Versions

```
read /iso/assemblies/user-service/versions
→ {
    "versions": ["2024.01.15", "2024.01.14", "2024.01.13"],
    "active": "2024.01.15"
  }
```

## Listing Assemblies

```
read /iso/assemblies
→ {
    "assemblies": ["user-service", "order-service", "gateway"]
  }
```

## Deactivating an Assembly

```
write /iso/assemblies/user-service/active {
  "version": null
}
```

This shuts down the running Assembly. The definitions remain stored.

## Deleting a Version

```
write /iso/assemblies/user-service/versions/2024.01.13 null
```

Deletes the version. Fails if the version is currently active.

## Structural Sharing

The runtime may implement structural sharing for efficiency:

```
Version 2024.01.14:
  gateway: sha256/abc123
  api: sha256/def456

Version 2024.01.15:
  gateway: sha256/abc123  ← Same hash, shared storage
  api: sha256/ghi789      ← New hash, new storage
```

Blocks with identical content hashes can share:

- Storage (the Wasm artifact is stored once)
- Running instances (during transitions, if safe)

This is an optimization, not a guarantee. Definitions behave as if everything
is independent.

## Export and Import

Because definitions are values, export/import is trivial:

**Export:**
```
read /iso/assemblies/user-service/versions/2024.01.15/definition > backup.yaml
```

**Import:**
```
write /iso/assemblies/user-service/versions/2024.01.15/definition < backup.yaml
```

**Clone to another system:**
```python
definition = read("/iso/assemblies/user-service/versions/2024.01.15/definition")
# ... connect to other system ...
write("/iso/assemblies/user-service/versions/2024.01.15/definition", definition)
```

The definition is self-contained. Block artifacts (Wasm files) must also be
available to the target system, either by copying or via a shared registry.

## Definition vs Configuration

An Assembly definition includes static configuration:

```yaml
config:
  api:
    timeout: 30s
    max_connections: 100
```

This config is part of the immutable definition. Changing it requires a new
version.

For values that change without redeployment (feature flags, dynamic settings),
reference an external config store:

```yaml
config:
  api:
    timeout: 30s                           # Static, in definition
    feature_flags: /services/config/api    # Dynamic, external reference
```

The Block reads `/services/config/api` at runtime. That path points to a
config store that can be updated independently of the Assembly version.

## Concurrent Versions

During transitions (blue-green, canary), multiple versions may run simultaneously:

```
read /iso/assemblies/user-service/active
→ {
    "version": "2024.01.15",
    "status": "active",
    "traffic": 90
  }

read /iso/assemblies/user-service/versions/2024.01.14/state
→ {
    "status": "draining",
    "traffic": 10
  }
```

The runtime manages traffic splitting. Both versions are fully instantiated
with their own Block instances.

## Relationship to Block Stores

From a Block's perspective, `/iso/assemblies/` is just another store wired into
its namespace. A Block could manage Assemblies by reading and writing these
paths—this is how deployment pipelines, operators, and management UIs work.

There's no privileged management API. If you can write to `/iso/assemblies/`,
you can deploy. Access control is capability-based: wire the path to grant
access, don't wire it to deny.

## Open Questions

1. **Garbage collection**: When are old versions deleted? Manual only, or
   automatic after some retention period?

2. **Artifact resolution**: How does the runtime resolve Block artifact paths
   (like `./gateway.wasm`) to actual content? Is there a standard registry
   protocol?

3. **Secrets**: How are secrets (TLS certs, API keys) injected? They shouldn't
   be in the definition. Is there a standard secrets store pattern?

4. **Multi-tenancy**: Can multiple tenants share a runtime with isolated
   `/iso/assemblies/` namespaces?
