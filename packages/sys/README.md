# structfs-sys

OS primitives exposed through StructFS paths.

This crate provides standard OS functionality through the StructFS read/write
interface, designed for environments where programs interact with the OS
exclusively through StructFS operations.

## Path Namespace

```
/sys/
    env/              # Environment variables (writes go to an in-process overlay)
    time/             # Clocks and sleep
    random/           # Random values
    proc/             # Process information
    fs/               # Filesystem operations
    docs/             # Documentation for this store
```

Reading the root of `SysStore` returns a map with each sub-store's root.

## Usage

```rust
use structfs_core_store::{path, NoCodec, Reader};
use structfs_sys::SysStore;

// Confine `fs` to one directory tree.
let mut store = SysStore::rooted("/srv/data")?;

// Read an environment variable.
let home = store.read(&path!("env/HOME"))?;

// Get the current time.
let now = store.read(&path!("time/now"))?.unwrap().into_value(&NoCodec)?;
```

### Exposure

`SysStore::new()` (and `FsStore::new()`) give **whole-filesystem access**:
whoever can write to the store can open, create, overwrite, and delete any
file the process can reach, and symlinks are followed. The `structfs` REPL
mounts it this way at `/ctx/sys`, because the REPL user is the process owner.

Anything less trusted should use `SysStore::rooted(dir)` or
`SysStore::with_fs(FsStore::rooted(dir)?)`. A rooted `fs` resolves relative
request paths against the root, accepts absolute paths only inside it, and
checks every path **after symlink resolution**: a symlink (or dangling
symlink) that leads outside the root is `PermissionDenied`. `unlink`, `rmdir`
and `rename` act on the directory entry itself, so removing an escaping
symlink removes only the link. The check happens at request time; another
process that swaps a directory for a symlink between check and use is outside
this guarantee.

`proc/self/cwd` writes change the process-wide working directory.

## Subsystems

### Environment (`env/`)

```bash
read env              # All variables (with the overlay applied) as a map
read env/HOME         # One variable as a string; absent variables read as null
write env/MY_VAR "x"  # Set a variable in the overlay
write env/MY_VAR null # Unset a variable in the overlay
```

Writes never mutate the process environment (`std::env::set_var` is undefined
behaviour while other threads may read the environment). They land in an
overlay private to the `EnvStore`; `proc/self/env` and child processes see the
real environment. `EnvStore::overrides()` exposes the overlay so a caller can
apply it when spawning a process.

### Time (`time/`)

```bash
read time                     # Every clock's current value
read time/now                 # Current time as an ISO 8601 / RFC 3339 string
read time/now_unix            # Unix timestamp in seconds
read time/now_unix_ms         # Unix timestamp in milliseconds
read time/monotonic           # Nanoseconds since the first TimeStore was created (unsigned)
write time/sleep {"ms": 100}  # Sleep; also accepts {"secs": N}
```

Negative or non-integer sleep amounts, and sleeps longer than one hour
(`MAX_SLEEP`), are `InvalidArgument`. A sleep blocks the calling thread.

### Random (`random/`)

```bash
read random/u64       # A random unsigned 64-bit integer
read random/uuid      # A random UUID v4 string
read random/bytes/16  # N random bytes as a bytes value (at most 1 MiB)
```

The store is read-only. `random/bytes/{n}` above `MAX_RANDOM_BYTES` (1 MiB) is
`ResourceLimit`.

### Process (`proc/`)

```bash
read proc/self/pid          # Process ID
read proc/self/cwd          # Current working directory
write proc/self/cwd "/tmp"  # Change the process-wide working directory
read proc/self/args         # Command-line arguments
read proc/self/exe          # Path to the executable
read proc/self/env          # The real process environment
```

Reading `proc` or `proc/self` returns every entry's value.

### Filesystem (`fs/`)

Actions are writes of a request map to an action path. `open` returns a
`handles/{id}` path for handle-based I/O; `stat` and `readdir` return a
`results/{id}` path to read the answer from. Ids are per store and never
reused within it; listings are in id order.

```bash
# Open a file; returns handles/{id}
write fs/open {"path": "/tmp/test.txt", "mode": "write", "encoding": "utf8"}

read fs/handles                        # Open handles, in id order
read fs/handles/0                      # Read from the cursor to EOF
write fs/handles/0 "Hello, World!"     # Write at the cursor
read fs/handles/0/at/10                # Read from byte 10 to EOF
read fs/handles/0/at/10/len/4          # Read at most 4 bytes from byte 10
write fs/handles/0/at/10 "text"        # Write from byte 10
read fs/handles/0/position             # The cursor position
write fs/handles/0/position {"pos": 0} # Seek
read fs/handles/0/meta                 # Size and type of the open file
write fs/handles/0/close null          # Close the handle
write fs/handles/0 null                # Also closes the handle (null deletes)

write fs/stat {"path": "/some/file"}   # Stat a path; returns results/{id}
write fs/readdir {"path": "/some/dir"} # List a directory; returns results/{id}
read fs/results/0                      # Read a stat or readdir answer
write fs/results/0 null                # Discard an answer

write fs/mkdir {"path": "/new/dir", "recursive": true}  # Create a directory
write fs/rmdir {"path": "/dir"}                         # Remove an empty directory
write fs/unlink {"path": "/file"}                       # Remove a file (or a symlink itself)
write fs/rename {"from": "/old", "to": "/new"}          # Rename

read fs/meta                           # Machine-readable schemas for every action
```

A `stat` answer is `{"size", "kind", "modified", "readonly"}`: `size` is an
unsigned byte count, `kind` is `file`, `dir`, `symlink` or `other` (symlinks
are followed, so `stat` reports the target), `modified` is an RFC 3339 string
or null where the platform has no modification time. A `readdir` answer is an
array of `{"name", "kind"}` entries sorted by name. The newest 256 answers are
retained (`DEFAULT_MAX_RESULTS`); older ones are evicted.

#### Open modes

| `mode` | Behaviour |
|---|---|
| `read` (default) | Open an existing file for reading |
| `write` | Create or truncate, then write |
| `append` | Create if missing, write at the end |
| `readwrite` | Create if missing, read and write without truncating |
| `create_new` (alias `createnew`) | Create a file that must not already exist |

#### Encodings

| `encoding` | Reads return | Writes accept |
|---|---|---|
| `base64` (default) | a base64 string | a base64 string |
| `utf8` (aliases `utf-8`, `text`) | a UTF-8 string; invalid UTF-8 is `InvalidArgument` | a string, taken verbatim |
| `bytes` (alias `raw`) | a bytes value | a string, taken verbatim |

Writes always accept a bytes value regardless of encoding. Unknown modes and
encodings are `InvalidArgument`; nothing falls back silently.

#### Limits and errors

- A single handle read returns at most 16 MiB (`DEFAULT_MAX_READ_LEN`,
  configurable with `FsStore::with_max_read_len`). A longer read, or an
  `at/{o}/len/{n}` with `n` above the cap, is `ResourceLimit`; nothing is
  allocated from an unchecked length. Page large files with
  `at/{offset}/len/{n}`.
- At most 256 handles may be open at once per store (`DEFAULT_MAX_HANDLES`,
  configurable with `FsStore::with_max_handles`); another `open` is
  `ResourceLimit` until a handle is closed.
- On a rooted store, `rmdir`, `unlink` and `rename` (either side) of the root
  itself are `PermissionDenied`.
- Negative seek positions are `InvalidArgument`.
- Unknown handles and results are `NotFound`; malformed requests are
  `InvalidArgument`; writes to read-only paths are `PermissionDenied`; OS
  failures are `Io`.

## Documentation (Docs Protocol)

This store implements the docs protocol, providing documentation at the `docs`
path. The operations it lists are the ones in this README (a test keeps the
two in step):

```bash
read docs           # Overview of sys store
read docs/env       # Environment variables
read docs/time      # Time operations
read docs/random    # Random values
read docs/proc      # Process information
read docs/fs        # Filesystem operations
```

The REPL's help store links mounted stores' docs, so `read /ctx/help/ctx/sys`
returns the same documentation.
