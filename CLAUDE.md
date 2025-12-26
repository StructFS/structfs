# Claude Code Instructions for StructFS

## Project Overview

StructFS is a Rust workspace that provides a uniform interface for accessing data through read/write operations on paths. The core philosophy is "everything is a store" - all data access, including mount management, HTTP requests, and configuration, happens through the same read/write interface.

## Workspace Structure

```
packages/
├── ll-store/    # Low-level byte stream traits
├── core-store/  # Core traits (Reader, Writer, Path, Value) and MountStore
├── serde-store/ # Serde integration for typed access
├── json_store/  # JSON-based in-memory store
├── http/        # HTTP client store, broker store
├── sys/         # OS primitives (env, time, proc, fs, random)
└── repl/        # Interactive REPL with syntax highlighting and completion
```

## Key Concepts

- **Stores**: Implement `Reader` and `Writer` traits for path-based data access
- **Value**: Core data type (Null, Bool, Integer, Float, String, Bytes, Array, Map)
- **Record**: Wrapper for raw bytes or parsed Value
- **MountStore**: Routes operations to different stores based on path prefixes
- **OverlayStore**: Mounts stores at paths, creating a unified tree
- **Broker pattern**: HTTP broker queues requests on write, executes on read
- **Docs protocol**: Stores can provide documentation at a `docs` path

## Development Commands

```bash
# Run all quality checks (format, clippy, tests)
./scripts/quality_gates.sh

# Individual commands
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Run the REPL
cargo run -p structfs-repl
```

## Code Style

- Keep solutions simple and focused - avoid over-engineering
- Prefer editing existing files over creating new ones
- Use `thiserror` for error types
- Use `serde` for serialization with JSON as the primary format

## Architecture Decisions

1. **Three-layer architecture**:
   - `ll-store`: Pure bytes, no semantics
   - `core-store`: Record/Value abstraction, path routing
   - `serde-store`: Serde integration for typed access

2. **Synchronous interface**: All store operations are synchronous. The HTTP broker uses a deferred execution pattern (write queues, read executes).

3. **Path-based routing**: Paths are the universal addressing mechanism. The `Path` type normalizes trailing slashes and validates components.

4. **Default context mounts**: The REPL provides built-in stores at `/ctx/*`:
   - `/ctx/http` - Async HTTP broker (background execution)
   - `/ctx/http_sync` - Sync HTTP broker (blocks until complete)
   - `/ctx/help` - Documentation system
   - `/ctx/sys` - OS primitives (env, time, proc, fs, random)

## Common Patterns

### Adding a new store type

1. Implement `Reader` and `Writer` traits from `structfs_core_store`
2. Add a variant to `MountConfig` in `core-store/src/mount_store.rs`
3. Handle the variant in `CoreReplStoreFactory` in `repl/src/store_context.rs`

### The HTTP broker pattern

```rust
// Write queues the request, returns handle path
write /ctx/http {"method": "GET", "path": "https://example.com"}
// Returns: /ctx/http/outstanding/0

// Read from handle executes the request
read /ctx/http/outstanding/0
// Returns: HttpResponse with status, headers, body
```

### The sys store

OS primitives exposed through paths:

```bash
read /ctx/sys/env/HOME           # Environment variables
read /ctx/sys/time/now           # Current time (ISO 8601)
read /ctx/sys/random/uuid        # Random UUID v4
read /ctx/sys/proc/self/pid      # Process ID
write /ctx/sys/fs/open {"path": "/tmp/file", "mode": "write"}  # File handles
```

### REPL Registers

Registers store command output for later use:

```bash
@handle write /ctx/sys/fs/open {"path": "/tmp/test", "mode": "write"}
write *@handle "Hello"           # Dereference to use as path
read @handle                     # Read register contents
```

## Testing

- Unit tests live alongside code in `#[cfg(test)]` modules
- REPL tests are mostly ignored (require interactive testing)

## Files to Know

- `packages/core-store/src/path.rs` - Path parsing and validation
- `packages/core-store/src/mount_store.rs` - MountConfig enum and mount management
- `packages/core-store/src/overlay_store.rs` - OverlayStore for composing stores
- `packages/http/src/core.rs` - HTTP broker store implementations
- `packages/sys/src/lib.rs` - SysStore with all sub-stores
- `packages/repl/src/store_context.rs` - REPL's store factory and default mounts
- `packages/repl/src/help_store.rs` - Help system
- `packages/repl/src/commands.rs` - Command parsing, register handling, dereference syntax
