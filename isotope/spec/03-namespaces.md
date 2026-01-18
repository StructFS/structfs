# Namespaces

A **Namespace** is a Block's view of the path hierarchy. Each Block has its
own namespace, and namespaces do not overlap.

## Definition

A namespace is a mapping from paths to stores. When a Block performs a read or
write, the path is resolved within its namespace to determine which store
handles the operation.

```
Block's Namespace:
  /              → root store (Assembly-provided)
  /input         → input store (mounted by Assembly)
  /output        → output store (mounted by Assembly)
  /ctx/iso       → system context store (always present)
  /services/db   → another Block's export (wired by Assembly)
```

## Namespace Isolation

Each Block's namespace is completely isolated:

- Block A's `/data` and Block B's `/data` are unrelated
- A Block cannot reference another Block's namespace
- There is no "global" namespace

This isolation is fundamental to Isotope's security model. A Block can only
access paths that have been explicitly mounted into its namespace by its
containing Assembly.

## Mount Points

A mount point binds a path prefix to a store:

```
mount /services/cache → cache_store
```

After this mount:
- `/services/cache` reads/writes go to `cache_store` at path `/`
- `/services/cache/hot/key` goes to `cache_store` at path `/hot/key`

### Mount Shadowing

If a path matches multiple mounts, the longest (most specific) match wins:

```
mount / → root_store
mount /services → services_store
mount /services/cache → cache_store

Path /services/cache/key → cache_store:/key
Path /services/db/query → services_store:/db/query
Path /data → root_store:/data
```

### Empty Mounts

A mount with no backing store returns "not found" for all reads and rejects
all writes. This can be used to block access to a path subtree:

```
mount /forbidden → (empty)
```

## The Root Mount

Every namespace has a root mount at `/`. This is the "default" store that
handles any path not matched by a more specific mount.

The root mount is typically an overlay store that composes:
- Input/output paths
- Configuration
- Context root
- Wired services

## System Context Mount

Every Block's namespace includes a mount at `/ctx/iso` (or similar conventional
path) for system services. This is how Blocks access:

- Process information (`/ctx/iso/proc`)
- Time (`/ctx/iso/time`)
- Random values (`/ctx/iso/random`)
- Inter-Block messaging (`/ctx/iso/ipc`)

See `04-context.md` for details on the context root.

## Namespace Operations

### List (Optional)

Some namespaces support listing paths:

```
list("/services") → ["cache", "db", "auth"]
```

Listing is optional. A namespace may:
- Support listing at all paths
- Support listing at some paths
- Not support listing at all

A namespace that doesn't support listing can still be read/written—the client
just needs to know the paths in advance.

### Watch (Optional)

Some namespaces support watching for changes:

```
watch("/input/events") → stream of changes
```

This is how Blocks can react to new data without polling. Watch semantics
are defined in `06-protocol.md`.

## Namespace Manipulation

A Block can manipulate its own namespace through the context root:

```
# Mount a new store at /temp
write /ctx/iso/ns/mount {"path": "/temp", "store": "memory://"}

# Unmount
write /ctx/iso/ns/unmount {"path": "/temp"}

# List current mounts
read /ctx/iso/ns/mounts
```

This allows Blocks to dynamically reconfigure their view of the world. An
Assembly can restrict this capability by not wiring `/ctx/iso/ns` to the Block.

## Plan 9 Comparison

Isotope namespaces are directly inspired by Plan 9 per-process namespaces:

| Plan 9 | Isotope |
|--------|---------|
| Per-process namespace | Per-Block namespace |
| `bind` / `mount` | Assembly wiring + dynamic mounts |
| Union directories | Overlay stores |
| `/dev`, `/proc`, `/net` | `/ctx/iso/*` |
| 9P file servers | StructFS stores |

The key difference: Plan 9 namespaces are built from files. Isotope namespaces
are built from stores, which are a superset (stores can represent things that
don't map well to files).

## Open Questions

1. **Relative paths**: Should namespaces support relative paths, or only
   absolute? If relative, what is the "current directory"?

2. **Symbolic links**: Should there be an equivalent to symlinks—paths that
   redirect to other paths?

3. **Namespace inheritance**: When a Block spawns a child (if that's even
   a concept), does the child inherit the parent's namespace?

4. **Namespace inspection**: Can a Block inspect its own namespace structure,
   or only access paths within it?
