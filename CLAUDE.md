# Claude Code Instructions for StructFS

## Project Overview

StructFS is a Rust workspace that provides a uniform interface for accessing data through read/write operations on paths. The core philosophy is "everything is a store" - all data access, including mount management, HTTP requests, and configuration, happens through the same read/write interface.

## Workspace Structure

```
packages/
├── store/       # Core traits (Reader, Writer, Path) and MountStore
├── json_store/  # JSON-based implementations (in-memory, local disk)
├── http/        # HTTP client store, broker store, remote StructFS client
├── sys/         # OS primitives (env, time, proc, fs, random)
└── repl/        # Interactive REPL with syntax highlighting and completion
```

## Key Concepts

- **Stores**: Implement `Reader` and `Writer` traits for path-based data access
- **MountStore**: Routes operations to different stores based on path prefixes
- **Overlay pattern**: Stores can be mounted at paths, creating a unified tree
- **Broker pattern**: HTTP broker queues requests on write, executes on read
- **Docs protocol**: Stores can provide documentation at a `docs` path; the help store mounts these for unified access
- **Cross-store access via mounting**: When a store needs to read from other stores, mount the relevant paths into it (no special traits needed)

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
- Dead code in `packages/store` should be preserved with `#[allow(dead_code)]`
- Use `thiserror` for error types
- Use `serde` for serialization with JSON as the primary format

## Architecture Decisions

1. **Synchronous interface**: All store operations are synchronous. The HTTP broker uses a deferred execution pattern (write queues, read executes).

2. **Path-based routing**: Paths are the universal addressing mechanism. The `Path` type normalizes trailing slashes and validates components.

3. **Mount management through stores**: Mounts are managed by writing to `/_mounts/*` paths, not through special APIs.

4. **Default context mounts**: The REPL provides built-in stores at `/ctx/*`:
   - `/ctx/http` - Async HTTP broker (background execution)
   - `/ctx/http_sync` - Sync HTTP broker (blocks until complete)
   - `/ctx/help` - Documentation system (with mounted store docs)
   - `/ctx/sys` - OS primitives (env, time, proc, fs, random)

5. **Docs protocol**: Stores expose documentation at their `docs` path. The HelpStore mounts these internally, so `read /ctx/help/sys` reads from the sys store's docs.

## Common Patterns

### Adding a new store type

1. Implement `Reader` and `Writer` traits from `structfs_store`
2. Add a variant to `MountConfig` in `mount_store.rs`
3. Handle the variant in `ReplStoreFactory` in `store_context.rs`

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

### Adding store documentation (docs protocol)

1. Create a `DocsStore` that returns documentation as JSON
2. Mount it at `docs` in your store (via OverlayStore)
3. The help store will mount it for unified access at `/ctx/help/<store-name>`

Example from sys store:
```rust
overlay.add_layer(Path::parse("docs").unwrap(), DocsStore::new());
```

## Testing

- Unit tests live alongside code in `#[cfg(test)]` modules
- Integration tests for `mount_store` are in `packages/json_store/tests/` to avoid circular dependencies
- REPL tests are mostly ignored (require interactive testing)

## Files to Know

- `packages/store/src/path.rs` - Path parsing and validation
- `packages/store/src/mount_store.rs` - MountConfig enum and mount management
- `packages/store/src/server.rs` - StoreRegistration for the docs protocol
- `packages/http/src/broker.rs` - HTTP broker store implementation
- `packages/sys/src/lib.rs` - SysStore composition via OverlayStore
- `packages/sys/src/docs.rs` - DocsStore for sys documentation (example of docs protocol)
- `packages/sys/src/fs/mod.rs` - File handle management with encoding support
- `packages/repl/src/store_context.rs` - REPL's store factory and default mounts
- `packages/repl/src/help_store.rs` - Help system with mounted store docs
- `packages/repl/src/commands.rs` - Command parsing, register handling, dereference syntax
