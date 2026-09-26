# StructFS

A uniform interface for accessing data through read/write operations on paths.

StructFS treats everything as a store - local files, remote APIs, in-memory
data, and even the mount configuration itself are all accessed through the same
read/write interface.

## Library quickstart

**0.4.0** is published for every workspace crate (see
[registry status](docs/release-status.md)). This checkout targets **0.5.0**
development, a breaking release guided by the
[2026-09-17 code audit](docs/code-audit-2026-09-17.md); its renames are in the
[0.5 migration guide](docs/migration-0.5.md). See also the
[0.4 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md)
and [platform matrix](https://github.com/StructFS/structfs/blob/main/docs/platforms.md).

The published quickstart uses:

```toml
[dependencies]
structfs = { version = "0.4.0", features = ["json"] }
```

```rust
use structfs::{path, MemoryStore, Reader, Record, Value, Writer};

fn main() -> Result<(), structfs::Error> {
    let mut store = MemoryStore::new();
    store.write(&path!("greeting"), Record::parsed(Value::String("hello".into())))?;
    assert!(store.read(&path!("greeting"))?.is_some());
    Ok(())
}
```

Default features expose only the core contract. On this 0.5 line the facade's
features are `async`, `typed`, `persist`, `net` (portable HTTP schema),
`net-blocking`, `net-streaming`, `os`, `handles`, `service`, `state` and
`profiles`; `full` enables them all. Each feature adds a module of the same
name (`structfs::typed`, `structfs::persist`, …), and the 0.4 feature/module
names `serde`, `json`, `http` and `sys` are gone so that `use structfs::*`
no longer shadows those crates. See the
[facade README](packages/structfs/README.md) for the full table and the
[0.5 migration guide](docs/migration-0.5.md) for the old-to-new mapping.
Rust 1.96+ is supported. Higher-level consistency, persistence, observation and
process guarantees belong to each store's documented contract.

See the [changelog](https://github.com/StructFS/structfs/blob/main/CHANGELOG.md),
migration guides for [0.5](docs/migration-0.5.md) (unreleased),
[0.4](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md),
[0.3](https://github.com/StructFS/structfs/blob/main/docs/migration-0.3.md) and
[0.2](https://github.com/StructFS/structfs/blob/main/docs/migration-0.2.md),
and the [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md).

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
- **Vi mode support**: `--vi`/`--emacs`, then `STRUCTFS_EDIT_MODE`, then a vi-family `EDITOR`/`VISUAL`, then `.inputrc`

## Packages

| Package | Description |
|---------|-------------|
| `structfs` | Feature-selectable library facade: core at the root, one module per feature |
| `structfs-service` | Shared async routing and owned provider lifetimes |
| `structfs-state` | Revisioned state, snapshots and bounded observation |
| `structfs-profiles` | Optional discovery, interactive and operation contracts |
| `structfs-handles` | Owned handles and bounded streaming primitives |
| `structfs-path-validation` / `structfs-path-macro` | Validated runtime and literal paths |
| `structfs-ll-store` | Low-level byte stream traits |
| `structfs-core-store` | Core traits (`Reader`, `Writer`, `Path`, `Value`) and mount system |
| `structfs-serde-store` | Serde integration for typed access |
| `structfs-json-store` | Durable snapshot (`BackedStore`) and append-log (`LogStore`) stores |
| `structfs-http` | HTTP client store and broker for arbitrary requests |
| `structfs-sys` | OS primitives (environment, time, filesystem, process, random) |
| `structfs-repl` | Interactive REPL with the `structfs` binary |
| `featherweight-runtime` | Strawman Isotope runtime: blocks, assemblies, namespaces, and the `/iso` store over StructFS |
| `featherweight-component` | WIT component-model binding adapter: run wasm components as Isotope blocks |
| `featherweight-guest` | Guest SDK and reference block for the core-wasm Block ABI binding |
| `featherweight-wasi` | WASI-over-Isotope shim core: the syscall surface implemented over the Block ABI |
| `featherweight` | Isotope runtime CLI (`fw`): run assemblies and the demo shell |
| `namecode` | Encode Unicode strings as valid programming-language identifiers (independently versioned; 0.1.1 published, the 0.5 cleanup's breaking namecode changes need a 0.2.0 release) |

## Store Types

Mount stores by writing configuration to `/ctx/mounts/<name>`:

```bash
# In-memory store (data lost on exit)
write /ctx/mounts/data {"type": "memory"}

# A JSON document on disk (saved after every write)
write /ctx/mounts/files {"type": "local", "path": "/path/to/data.json"}

# HTTP client with base URL
write /ctx/mounts/api {"type": "http", "url": "https://api.example.com"}

# HTTP broker for arbitrary URLs
write /ctx/mounts/http {"type": "httpbroker"}
```

The full list of mount types is in the
[REPL README](packages/repl/README.md#mount-types).

## HTTP Broker

Two HTTP brokers make requests to any URL. The sync broker at
`/ctx/http_sync` queues a request on write and executes it when you read the
handle:

```bash
# Queue a request (returns a handle path)
> write /ctx/http_sync {"method": "GET", "path": "https://httpbin.org/get"}
ok
→ /ctx/http_sync/outstanding/0

# Execute by reading from the handle (blocks; the response is cached)
> read /ctx/http_sync/outstanding/0
{"status": 200, "headers": {...}, "body": {...}}

# POST with headers and body
> write /ctx/http_sync {"method": "POST", "path": "https://httpbin.org/post", "headers": {"Authorization": "Bearer token"}, "body": {"key": "value"}}
```

The background broker at `/ctx/http` starts the request as soon as it is
written. Reading the handle reports its status; `response/wait` parks until
the response arrives:

```bash
> write /ctx/http {"method": "GET", "path": "https://httpbin.org/get"}
ok
→ /ctx/http/outstanding/0
> read /ctx/http/outstanding/0                 # request status
> read /ctx/http/outstanding/0/response/wait   # the response, once ready
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

The REPL includes a help system at `/ctx/help`. Each mounted store that serves
`docs` becomes a topic keyed by its mount path:

```bash
read /ctx/help                        # List topics
read /ctx/help/ctx/repl               # REPL docs
read /ctx/help/ctx/repl/commands      # Available commands
read /ctx/help/ctx/repl/mounts        # Mount system and mount types
read /ctx/help/ctx/repl/registers     # Register usage
read /ctx/help/ctx/http               # Background HTTP broker
read /ctx/help/ctx/http_sync          # Sync HTTP broker
read /ctx/help/ctx/sys                # System primitives
read /ctx/help/search/broker          # Search every topic
```

## Development

```bash
# Run quality checks (format, clippy, tests, 90% coverage gate)
./scripts/quality_gates.sh

# Run tests only
cargo test --workspace --all-features --locked

# Build release
cargo build --release
```

The standalone consumer fixtures under `tests/` are exercised by
`scripts/check-release.sh` and `scripts/check-featherweight-release.py`; see
[docs/releasing.md](docs/releasing.md).

## License

See [LICENSE](LICENSE) for details.
