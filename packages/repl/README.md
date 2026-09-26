# structfs-repl

Interactive REPL for StructFS.

## Installation

```bash
cargo install --path packages/repl
# or
cargo run -p structfs-repl
```

## Usage

```bash
$ structfs

  _____ _                   _   _____ ____
 / ____| |                 | | |  ___/ ___|
| (___ | |_ _ __ _   _  ___| |_| |_  \___ \
 \___ \| __| '__| | | |/ __| __|  _|  ___) |
 ____) | |_| |  | |_| | (__| |_| |   |____/
|_____/ \__|_|   \__,_|\___|\___|_|

Type 'help' for available commands, 'exit' to quit.

6 mount(s) / >
```

Pass `--vi` or `--emacs` to choose the line-editing mode; otherwise
`STRUCTFS_EDIT_MODE`, then a vi-family `EDITOR`/`VISUAL`, then the inputrc's
`set editing-mode` directive decide.

## Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `read [path\|@reg]` | `get`, `r` | Read the value at a path or register |
| `write <path> <json\|@reg>` | `set`, `w` | Write a value to a path or register |
| `ls [path]` | | List the child names at a path |
| `cd <path>` | | Change the current path |
| `pwd` | | Print the current path |
| `mounts` | | List mounts and their configs (read /ctx/mounts) |
| `registers` | `regs` | List all registers |
| `help [topic]` | `?` | Show help (try: help ctx/http) |
| `exit` | `quit`, `q` | Exit the REPL |

## Default Mounts

The REPL starts with these mounts (this table is checked against the code):

| Path | Description |
|------|-------------|
| `/ctx/repl` | REPL documentation |
| `/ctx/http` | HTTP broker (background threads) |
| `/ctx/http_sync` | HTTP broker (sync, blocks on read) |
| `/ctx/sys` | System primitives (env, time, random, proc, fs); unconfined filesystem access |
| `/ctx/registers` | Registers (session-local named values) |
| `/ctx/help` | Documentation system |

`/ctx/sys` can read and write any file the REPL process can: it is not
confined to a directory. Embedders that need confinement use
`structfs_sys::SysStore::rooted`.

## Mount types

Write a config to `/ctx/mounts/<name>` to mount a store at `/<name>`;
write `null` to unmount.

| `type` | Fields | Store |
|--------|--------|-------|
| `memory` | | In-memory store (lost on exit) |
| `local` | `path` | A JSON document on disk, saved atomically after every write |
| `http` | `url` | HTTP client for a base URL: read is GET, write is POST |
| `httpbroker` | | Sync HTTP request broker |
| `backgroundhttpbroker` | | Background HTTP request broker (`asynchttpbroker` is accepted as an alias) |
| `log` | `path` | A JSONL append log; page with `entries/from/{n}` |
| `recording` | `path` | A recorded `fw` run's directory, read-only |
| `sys`, `help`, `repl`, `registers` | | The built-in stores |

## Examples

```bash
# Create an in-memory store
> write /ctx/mounts/data {"type": "memory"}
ok

# Write some data
> write /data/users/1 {"name": "Alice", "email": "alice@example.com"}
ok

# Read it back
> read /data/users/1
{
  "email": "alice@example.com",
  "name": "Alice"
}

# Make an HTTP request with the sync broker: write queues, read executes
> write /ctx/http_sync {"method": "GET", "path": "https://httpbin.org/get"}
ok
→ /ctx/http_sync/outstanding/0

> read /ctx/http_sync/outstanding/0
{
  "status": 200,
  "body": {...}
}

# The background broker at /ctx/http starts the request on write; reading
# the handle returns its status, and response/wait parks until it is done
> @req write /ctx/http {"method": "GET", "path": "https://httpbin.org/get"}
> read *@req
> read *@req/response/wait

# Get help: topics are keyed by mount path
> read /ctx/help
> read /ctx/help/ctx/http
> read /ctx/help/ctx/repl/commands
```

## Registers

Registers store command output for later use:

```bash
# Capture output to a register
> @result read /ctx/sys/time/now

# Read from register
> read @result
"2024-01-15T10:30:00Z"

# Dereference register to use its value as a path
> @handle write /ctx/sys/fs/open {"path": "/tmp/file", "mode": "read"}
> read *@handle          # Uses handle value as path
> read /foo/*@handle     # Interpolation anywhere in path

# List all registers
> registers
```

## Features

- **Syntax highlighting**: JSON is highlighted as you type
- **Tab completion**: Complete commands with Tab
- **History**: Command history persisted across sessions
- **Registers**: Store and reuse command output with `@name` and `*@name`
- **Vi mode**: `--vi`/`--emacs`, then `STRUCTFS_EDIT_MODE`, then a vi-family `EDITOR`/`VISUAL`, then `.inputrc`

## Path Syntax

| Syntax | Description |
|--------|-------------|
| `/foo/bar` | Absolute path from root |
| `foo/bar` | Relative to current directory |
| `..` | Parent directory |
| `../foo` | Relative path going up |
| `/` | Root |

Trailing slashes are normalized (`/foo/` = `/foo`).
