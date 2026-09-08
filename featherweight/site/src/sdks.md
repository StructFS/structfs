---
layout: base.njk
title: SDKs
permalink: /sdks/
---

<div class="doc-page">

# SDKs

A block's entire interface is the two-function [Block
ABI]({{ sites.isotope }}/abi/), so an SDK is small by
construction: wrap `read` and `write`, export `block_alloc`,
`manifest`, and `run`. These exist today:

## Rust — `featherweight-guest`

The reference SDK and the reference kv block, in one crate. The `sdk`
module is the whole ABI surface; everything else is ordinary library
code.

```rust
use featherweight_guest::sdk::{structfs_read, structfs_write};

let bytes = structfs_read("iso/env")?;          // Ok(Some(bytes)) | Ok(None)
structfs_write("iso/stdio/stdout", b"hello")?;  // Ok(result path)
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release -p featherweight-guest
```

No componentization, no bindgen — the artifact runs under the native
runtime and the [browser host](/demo/) unmodified.

## AssemblyScript

A [~60-line reference
SDK](https://github.com/structfs/structfs/tree/main/featherweight/sdk/assemblyscript)
demonstrating that the binding is an afternoon's work in any language
that targets wasm: two `@external` declarations, a `block_alloc`
export, and byte-buffer helpers.

## JavaScript — the browser host

The host side of the ABI, for running blocks rather than writing them:
[~120 lines of dependency-free ES
modules](https://github.com/structfs/structfs/tree/main/featherweight/host/browser)
that execute the same guest binaries wasmtime runs — batch mode
anywhere, resident mode (a worker parked in `Atomics.wait`) wherever
COOP/COEP headers allow `SharedArrayBuffer`. [It powers the
demo.](/demo/)

## POSIX programs — `featherweight-wasi`

Not an SDK but a compatibility layer: the WASI syscall surface
implemented over the same store paths, so programs written against
libc-style interfaces run without knowing Isotope exists. Args, env,
clocks, random, stdio, exit codes, and file descriptors over the
byte-stream pattern.

## Crates

The Rust crates — `featherweight` (the `fw` CLI),
`featherweight-runtime`, `featherweight-component`,
`featherweight-wasi`, and `featherweight-guest` — are being prepared
for a 0.2.0 release to crates.io alongside the `structfs-*` stack.
Until then, build from the
[repository](https://github.com/StructFS/structfs).

</div>
