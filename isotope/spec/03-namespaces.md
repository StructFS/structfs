# Namespaces

A **Namespace** is a Block's view of the path hierarchy. Each Block has its
own namespace, and namespaces do not overlap.

## Definition

A namespace is a mapping from paths to stores. When a Block performs a read or
write, the path is resolved within its namespace to determine which store
handles the operation.

```
Block's Namespace:
  /iso/           → Isotope system services (always present)
  /services/cache → Another Block (wired by Assembly)
  /services/db    → Another Block (wired by Assembly)
  /config/        → Configuration store (provided by Assembly)
```

## Namespace Isolation

Each Block's namespace is completely isolated:

- Block A's `/data` and Block B's `/data` are unrelated
- A Block cannot reference another Block's namespace
- There is no "global" namespace

This isolation is fundamental to Isotope's security model. A Block can only
access paths that have been explicitly wired into its namespace by its
containing Assembly.

## The `/iso/` Root

Every Block's namespace includes `/iso/`, the Isotope system namespace. This
is provided by the runtime and contains:

```
/iso/
├── server/             # Server Protocol interface
│   ├── requests        # Read incoming requests
│   └── requests/pending # Read batch of pending requests
├── self/               # Block identity
│   ├── id              # Unique identifier
│   ├── state           # Lifecycle state
│   └── interface       # Write interface declaration
├── shutdown/           # Lifecycle control
│   ├── requested       # Check if shutdown requested
│   └── complete        # Signal shutdown complete
├── time/               # Time services
│   ├── now             # Current time
│   └── monotonic       # Monotonic counter
├── random/             # Randomness
│   ├── uuid            # Random UUID
│   └── bytes/{n}       # Random bytes
└── log/                # Logging
    ├── debug
    ├── info
    ├── warn
    └── error
```

The `/iso/` prefix is **reserved**. Assemblies cannot wire paths under `/iso/`.
This ensures every Block has reliable access to system services.

## Wired Paths

Everything outside `/iso/` is wired by the Assembly:

```yaml
# Assembly wiring
wiring:
  api:/services/cache -> cache
  api:/services/db -> database
  api:/config -> $config
```

The `api` Block's namespace becomes:

```
/iso/           → runtime-provided
/services/cache → cache Block
/services/db    → database Block
/config         → config store
```

## Path Rewriting

When a path is wired to another Block, the mount prefix is stripped:

```
Block A's perspective:     /services/cache/users/123
Wiring:                    /services/cache -> cache Block
Cache Block receives:      /users/123
```

The target Block sees paths relative to its own root. It doesn't know where
it's mounted in the caller's namespace.

This is essential for location transparency. A Block's code is the same
regardless of where it's wired.

## Mount Shadowing

If a path matches multiple wirings, the longest (most specific) match wins:

```yaml
wiring:
  api:/services -> services_block
  api:/services/cache -> cache_block
```

Resolution:
- `/services/cache/key` → routes to `cache_block` (longer match)
- `/services/db/query` → routes to `services_block` (falls through)

## Empty Paths

A path with no wiring returns "not found" for reads and rejects writes:

```
read("/nonexistent/path") → null (not found)
write("/nonexistent/path", data) → error (no handler)
```

## Namespace Operations

### List (Optional)

Some stores support listing child paths:

```
read("/services/") → ["cache", "db"]
```

Whether a path is listable depends on the underlying store. Not all stores
support listing.

### Meta Lens

The StructFS meta lens pattern applies within namespaces:

```
read("/services/cache/meta/users/123") → {
  "readable": true,
  "writable": true,
  ...
}
```

This provides introspection about path capabilities.

## Namespace Manipulation

Blocks cannot modify their own namespace. The namespace is fixed by the
Assembly at startup.

This is intentional: it prevents Blocks from escalating their own capabilities.
A Block can only access what the Assembly explicitly grants.

Future extensions might allow controlled namespace manipulation (e.g., a Block
requesting additional capabilities from the runtime), but this would require
explicit permission from the Assembly.

## Plan 9 Comparison

Isotope namespaces are inspired by Plan 9 per-process namespaces:

| Plan 9 | Isotope |
|--------|---------|
| Per-process namespace | Per-Block namespace |
| `bind` / `mount` | Assembly wiring |
| Union directories | Not supported (longest match) |
| `/dev`, `/proc`, `/net` | `/iso/*` |
| 9P file servers | StructFS stores |

Key differences:

1. **No dynamic binding**: In Plan 9, a process can modify its own namespace.
   In Isotope, namespaces are fixed by the Assembly.

2. **No union mounts**: Plan 9 allows multiple file servers at the same path.
   Isotope uses longest-prefix matching instead.

3. **Stores, not files**: Isotope routes to StructFS stores (which can represent
   things beyond files), not file servers.

## Open Questions

1. **Relative paths**: Should namespaces support relative paths (e.g., `./foo`),
   or only absolute paths from root?

2. **Symbolic links**: Should there be an equivalent to symlinks—paths that
   redirect to other paths within the same namespace?

3. **Namespace inspection**: Can a Block enumerate its own namespace structure,
   or only access paths it already knows about?
