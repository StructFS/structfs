# AssemblyScript SDK (core binding)

The complete StructFS ABI surface for an AssemblyScript block, per
`isotope/spec/11-core-wasm-binding.md`: two imports, one `block_alloc`
export, and safe `read`/`write` wrappers — about sixty lines, no
bindgen, no component tooling.

A block adds its own `manifest(ret: usize): i32` (fill the ret record
with JSON bytes, return 0) and `run(): i32` (the exit code), then
builds with plain `asc`. The runtime loads the resulting core module
directly.

Status: reference implementation, written to the spec's contract;
not yet exercised by this repo's CI (no AssemblyScript toolchain in
the build). The Rust equivalent (`featherweight/guest`) is the
CI-covered twin.
