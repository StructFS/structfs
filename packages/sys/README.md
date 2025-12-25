# structfs-sys

OS primitives exposed through StructFS paths.

This crate provides standard OS functionality through the StructFS read/write
interface, designed for environments where programs interact with the OS
exclusively through StructFS operations.

## Path Namespace

```
/sys/
    env/              # Environment variables
    time/             # Clocks and sleep
    random/           # Random number generation
    proc/             # Process information
    fs/               # Filesystem operations
    docs/             # Documentation for this store
```

## Usage

```rust
use structfs_sys::SysStore;
use structfs_store::{Reader, Writer, Path};

let mut store = SysStore::new();

// Read environment variable
let home: Option<String> = store.read_owned(&Path::parse("env/HOME").unwrap()).unwrap();

// Get current time
let now: Option<String> = store.read_owned(&Path::parse("time/now").unwrap()).unwrap();

// Read documentation
let docs: Option<serde_json::Value> = store.read_owned(&Path::parse("docs").unwrap()).unwrap();
```

## Subsystems

### Environment (`env/`)

```bash
read env              # List all environment variables as object
read env/HOME         # Read specific variable (string or null)
write env/MY_VAR "x"  # Set environment variable
write env/MY_VAR null # Unset environment variable
```

### Time (`time/`)

```bash
read time/now         # Current time (ISO 8601)
read time/now_unix    # Unix timestamp (seconds)
read time/now_unix_ms # Unix timestamp (milliseconds)
read time/monotonic   # Monotonic clock (nanoseconds)
write time/sleep {"ms": 100}  # Sleep for 100ms
```

### Random (`random/`)

```bash
read random/u64       # Random 64-bit integer
read random/uuid      # Random UUID v4
write random/bytes {"count": 16}  # Get N random bytes (base64)
```

### Process (`proc/`)

```bash
read proc/self/pid    # Current process ID
read proc/self/cwd    # Current working directory
read proc/self/args   # Command-line arguments
read proc/self/exe    # Path to executable
read proc/self/env    # All environment variables
```

### Filesystem (`fs/`)

File operations use a handle-based pattern:

```bash
# Open a file (returns handle path like "handles/1")
write fs/open {"path": "/tmp/test.txt", "mode": "write", "encoding": "utf8"}

# Write through handle
write fs/handles/1 "Hello, World!"

# Close handle
write fs/handles/1/close null

# Other operations
write fs/stat {"path": "/some/file"}
write fs/mkdir {"path": "/new/dir"}
write fs/rmdir {"path": "/dir"}
write fs/unlink {"path": "/file"}
write fs/rename {"from": "/old", "to": "/new"}
write fs/readdir {"path": "/dir"}
```

#### File Modes

- `read` - Open for reading (file must exist)
- `write` - Open for writing (truncates if exists, creates if not)
- `append` - Open for appending
- `readwrite` - Open for both reading and writing
- `create_new` - Create new file (fails if exists)

#### Encodings

- `base64` - Default, binary-safe
- `utf8` - UTF-8 text (errors on invalid sequences)
- `latin1` - ISO-8859-1 (any byte sequence valid)
- `ascii` - ASCII only (errors on bytes > 127)

## Documentation (Docs Protocol)

This store implements the docs protocol, providing documentation at the `docs`
path:

```bash
read docs           # Overview of sys store
read docs/env       # Environment variables help
read docs/time      # Time operations help
read docs/fs        # Filesystem help
```

The REPL's help store mounts this, so `read /ctx/help/sys` returns the same
documentation.
