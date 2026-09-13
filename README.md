# StructFS

A uniform interface for accessing data through read/write operations on paths.

StructFS treats everything as a store - local files, remote APIs, in-memory
data, and even the mount configuration itself are all accessed through the same
read/write interface.

## Library quickstart

Published baseline: **0.2.0**. This checkout targets **0.3.0 (unreleased)**;
see the [0.3 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.3.md)
and [platform matrix](https://github.com/StructFS/structfs/blob/main/docs/platforms.md).

The published quickstart uses:

```toml
[dependencies]
structfs = { version = "0.2.0", features = ["json"] }
structfs-core-store = "0.2.0" # required by the path! expansion
```

```rust
use structfs::{path, InMemoryStore, Reader, Record, Value, Writer};

fn main() -> Result<(), structfs::Error> {
    let mut store = InMemoryStore::new();
    store.write(&path!("greeting"), Record::parsed(Value::String("hello".into())))?;
    assert!(store.read(&path!("greeting"))?.is_some());
    Ok(())
}
```

Default features expose only the core contract. Opt into `serde`, `json`, `http`,
`sys`, `async`, `service`, `state`, or `profiles`; `full` enables them all.
Rust 1.96+ is supported. Higher-level consistency, persistence, observation and
process guarantees belong to each store's documented contract.

See the [changelog](https://github.com/StructFS/structfs/blob/main/CHANGELOG.md),
[migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.2.md),
and [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md).

## REPL quickstart

```bash
# Build and run the REPL
cargo run -p structfs-repl

# Inside the REPL:
> write /ctx/mounts/data {"type": "memory"}
> write /data/users/1 {"name": "Alice", "email": "alice@example.com"}
> read /data/users/1
> read /ctx/help
```

## Features

- **Unified data access**: Everything is a path-based store
- **Mount system**: Combine multiple stores into a single tree
- **HTTP broker**: Make HTTP requests to any URL through read/write
- **System primitives**: Access OS functionality (env, time, fs, proc, random) through paths
- **Interactive REPL**: Explore stores with syntax highlighting and tab completion
- **Registers**: Store command output and dereference paths with `@name` and `*@name`
- **Vi mode support**: Detected from EDITOR, .inputrc, or STRUCTFS_EDIT_MODE

## Packages

| Package | Description |
|---------|-------------|
| `structfs` | Feature-selectable library facade |
| `structfs-service` | Shared async routing and owned provider lifetimes |
| `structfs-state` | Revisioned state, snapshots and bounded observation |
| `structfs-profiles` | Optional discovery, interactive and operation contracts |
| `structfs-handles` | Owned handles and bounded streaming primitives |
| `structfs-path-validation` / `structfs-path-macro` | Validated runtime and literal paths |
| `structfs-ll-store` | Low-level byte stream traits |
| `structfs-core-store` | Core traits (`Reader`, `Writer`, `Path`, `Value`) and mount system |
| `structfs-serde-store` | Serde integration for typed access |
| `structfs-json-store` | JSON-based in-memory store |
| `structfs-http` | HTTP client store and broker for arbitrary requests |
| `structfs-sys` | OS primitives (environment, time, filesystem, process, random) |
| `structfs-repl` | Interactive REPL with the `structfs` binary |

## Store Types

Mount stores by writing configuration to `/ctx/mounts/<name>`:

```bash
# In-memory store (data lost on exit)
write /ctx/mounts/data {"type": "memory"}

# Local filesystem (persisted as JSON files)
write /ctx/mounts/files {"type": "local", "path": "/path/to/dir"}

# HTTP client with base URL
write /ctx/mounts/api {"type": "http", "url": "https://api.example.com"}

# HTTP broker for arbitrary URLs
write /ctx/mounts/http {"type": "httpbroker"}

# Remote StructFS server
write /ctx/mounts/remote {"type": "structfs", "url": "https://structfs.example.com"}
```

## HTTP Broker

The HTTP broker at `/ctx/http` allows making requests to any URL:

```bash
# Queue a request (returns a handle path)
> write /ctx/http {"method": "GET", "path": "https://httpbin.org/get"}
Written to: /ctx/http/outstanding/0

# Execute by reading from the handle
> read /ctx/http/outstanding/0
{"status": 200, "headers": {...}, "body": {...}}

# POST with headers and body
> write /ctx/http {"method": "POST", "path": "https://httpbin.org/post", "headers": {"Authorization": "Bearer token"}, "body": {"key": "value"}}
```

## System Primitives

The `/ctx/sys` store exposes OS functionality through paths:

```bash
# Environment variables
> read /ctx/sys/env/HOME
"/Users/alice"

# Time operations
> read /ctx/sys/time/now
"2024-01-15T10:30:00Z"

# Random values
> read /ctx/sys/random/uuid
"550e8400-e29b-41d4-a716-446655440000"

# Process info
> read /ctx/sys/proc/self/pid
12345

# File operations (with handles)
> @h write /ctx/sys/fs/open {"path": "/tmp/test.txt", "mode": "write", "encoding": "utf8"}
> write *@h "Hello, World!"
> write *@h/close null
```

## Registers

Store command output in registers for later use:

```bash
# Capture output to a register
> @result read /ctx/sys/time/now

# Read from register
> read @result
"2024-01-15T10:30:00Z"

# Dereference register to use as path
> @handle write /ctx/sys/fs/open {"path": "/tmp/file", "mode": "read"}
> read *@handle

# List all registers
> registers
```

## Built-in Help

The REPL includes a help system at `/ctx/help`:

```bash
read /ctx/help           # Overview
read /ctx/help/commands  # Available commands
read /ctx/help/mounts    # Mount system docs
read /ctx/help/http      # HTTP broker usage
read /ctx/help/stores    # Store type reference
read /ctx/help/sys       # System primitives (from sys store docs)
read /ctx/help/registers # Register usage
```

## Development

```bash
# Run quality checks (format, lint, test)
./scripts/quality_gates.sh

# Run tests only
cargo test --workspace

# Build release
cargo build --release
```

## License

See [LICENSE](LICENSE) for details.
