# Claude Code Instructions for StructFS

## Project Overview

StructFS is a Rust workspace that provides a uniform interface for accessing data through read/write operations on paths. The core philosophy is "everything is a store" - all data access, including mount management, HTTP requests, and configuration, happens through the same read/write interface.

## Workspace Structure

```
packages/
├── store/       # Core traits (Reader, Writer, Path) and MountStore
├── json_store/  # JSON-based implementations (in-memory, local disk)
├── http/        # HTTP client store, broker store, remote StructFS client
└── repl/        # Interactive REPL with syntax highlighting and completion
```

## Key Concepts

- **Stores**: Implement `Reader` and `Writer` traits for path-based data access
- **MountStore**: Routes operations to different stores based on path prefixes
- **Overlay pattern**: Stores can be mounted at paths, creating a unified tree
- **Broker pattern**: HTTP broker queues requests on write, executes on read

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

4. **Default context mounts**: The REPL provides `/ctx/http` (HTTP broker) and `/ctx/help` (documentation) by default.

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

## Testing

- Unit tests live alongside code in `#[cfg(test)]` modules
- Integration tests for `mount_store` are in `packages/json_store/tests/` to avoid circular dependencies
- REPL tests are mostly ignored (require interactive testing)

## Files to Know

- `packages/store/src/path.rs` - Path parsing and validation
- `packages/store/src/mount_store.rs` - MountConfig enum and mount management
- `packages/http/src/broker.rs` - HTTP broker store implementation
- `packages/repl/src/store_context.rs` - REPL's store factory and default mounts
- `packages/repl/src/help_store.rs` - In-REPL documentation system
