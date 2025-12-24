# StructFS

A uniform interface for accessing data through read/write operations on paths.

StructFS treats everything as a store - local files, remote APIs, in-memory
data, and even the mount configuration itself are all accessed through the same
read/write interface.

## Quick Start

```bash
# Build and run the REPL
cargo run -p structfs-repl

# Inside the REPL:
> write /_mounts/data {"type": "memory"}
> write /data/users/1 {"name": "Alice", "email": "alice@example.com"}
> read /data/users/1
> read /ctx/help
```

## Features

- **Unified data access**: Everything is a path-based store
- **Mount system**: Combine multiple stores into a single tree
- **HTTP broker**: Make HTTP requests to any URL through read/write
- **Interactive REPL**: Explore stores with syntax highlighting and tab completion
- **Vi mode support**: Detected from EDITOR, .inputrc, or STRUCTFS_EDIT_MODE

## Packages

| Package | Description |
|---------|-------------|
| `structfs-store` | Core traits (`Reader`, `Writer`, `Path`) and mount system |
| `structfs-json-store` | JSON-based stores (in-memory, local filesystem) |
| `structfs-http` | HTTP client store and broker for arbitrary requests |
| `structfs-repl` | Interactive REPL with the `structfs` binary |

## Store Types

Mount stores by writing configuration to `/_mounts/<name>`:

```bash
# In-memory store (data lost on exit)
write /_mounts/data {"type": "memory"}

# Local filesystem (persisted as JSON files)
write /_mounts/files {"type": "local", "path": "/path/to/dir"}

# HTTP client with base URL
write /_mounts/api {"type": "http", "url": "https://api.example.com"}

# HTTP broker for arbitrary URLs
write /_mounts/http {"type": "httpbroker"}

# Remote StructFS server
write /_mounts/remote {"type": "structfs", "url": "https://structfs.example.com"}
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

## Built-in Help

The REPL includes a help system at `/ctx/help`:

```bash
read /ctx/help           # Overview
read /ctx/help/commands  # Available commands
read /ctx/help/mounts    # Mount system docs
read /ctx/help/http      # HTTP broker usage
read /ctx/help/stores    # Store type reference
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
