# featherweight-wasi

The WASI-over-Isotope shim core
([spec 10](https://github.com/StructFS/structfs/blob/main/isotope/spec/10-wasi-tower.md)).

Isotope does not depend on WASI; WASI is a compatibility layer that
bottoms out in the Block ABI's two functions. This crate implements the
syscall surface **generically over any StructFS store** — the block's
namespace on a real runtime, a fake in tests — so every "syscall" is
store traffic on the `/iso/` surface and the runtime never learns WASI
exists.

## What's implemented

- **Identity**: `args`, `environ` from `iso/self/args` and `iso/env`.
- **Clocks and randomness**: `clock_time_get` (realtime, monotonic),
  `random_get`, clock-only `poll_oneoff` sleeping via `iso/time`.
- **Stdio**: `fd_write` to `iso/stdio/stdout`/`stderr`; `fd_read` on
  fd 0 with `read(2)` semantics over the line-oriented stdin surface.
- **Process**: `proc_exit` writing `iso/shutdown/complete`, plus the
  errno mapping for the whole typed StructFS error taxonomy
  (`NotFound` → `ENOENT`, `PermissionDenied` → `ENOTCAPABLE`, …).
- **File descriptors** over the
  [byte-stream pattern](https://github.com/StructFS/structfs/blob/main/docs/patterns/bytestream.md):
  preopens are namespace mounts; `path_open`/`fd_read`/`fd_write`/
  `fd_seek`/`fd_filestat`/`fd_close` compile down to ranged reads
  (`{file}/at/{offset}/len/{n}`), positioned and append writes, and the
  store conventions (`O_TRUNC` is a `Null` delete). POSIX names cross
  into path components losslessly via Namecode, and `..` can never
  escape a preopen.

`MemFiles` ships as an in-memory reference store for the byte-stream
pattern, useful as a preopen target in tests.

## Example

```rust,ignore
use featherweight_wasi::{OpenFlags, WasiIso};
use structfs_core_store::path;

let mut wasi = WasiIso::with_preopens(
    namespace, // anything implementing Reader + Writer
    vec![("/data".to_string(), path!("files"))],
);
let fd = wasi.path_open(3, "notes.txt", OpenFlags {
    write: true, create: true, ..Default::default()
})?;
wasi.fd_write(fd, b"hello")?;
wasi.fd_close(fd)?;
# Ok::<(), featherweight_wasi::Errno>(())
```

The wasm packaging (the same core behind `wasi_snapshot_preview1`
exports, composed onto stock binaries at load time) is the thin outer
layer; this crate is the portable middle developed and verified
natively, without a wasm toolchain.
