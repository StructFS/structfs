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

2 mount(s) /[I]>
```

## Commands

| Command | Aliases | Description |
|---------|---------|-------------|
| `read [path]` | `get`, `r` | Read and display JSON at path |
| `write <path> <json>` | `set`, `w` | Write JSON to path |
| `cd <path>` | | Change current directory |
| `pwd` | | Print current directory |
| `mounts` | `ls` | List current mounts |
| `help` | `?` | Show help |
| `exit` | `quit`, `q` | Exit the REPL |

## Default Mounts

The REPL starts with these mounts:

| Path | Description |
|------|-------------|
| `/ctx/http` | HTTP broker for making requests to any URL |
| `/ctx/help` | Built-in documentation system |

## Examples

```bash
# Create an in-memory store
> write /_mounts/data {"type": "memory"}
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

# Make an HTTP request
> write /ctx/http {"method": "GET", "path": "https://httpbin.org/get"}
ok
result path: /ctx/http/outstanding/0
(read from this path to get the result)

> read /ctx/http/outstanding/0
{
  "status": 200,
  "body": {...}
}

# Get help
> read /ctx/help/http
```

## Features

- **Syntax highlighting**: JSON is highlighted as you type
- **Tab completion**: Complete commands with Tab
- **History**: Command history persisted across sessions
- **Vi mode**: Automatically detected from EDITOR, .inputrc, or STRUCTFS_EDIT_MODE

## Path Syntax

| Syntax | Description |
|--------|-------------|
| `/foo/bar` | Absolute path from root |
| `foo/bar` | Relative to current directory |
| `..` | Parent directory |
| `../foo` | Relative path going up |
| `/` | Root |

Trailing slashes are normalized (`/foo/` = `/foo`).
