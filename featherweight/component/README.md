# featherweight-component

The WIT component-model binding adapter for
[featherweight-runtime](https://crates.io/crates/featherweight-runtime).

The featherweight core knows only the Isotope Block ABI
([spec 10](https://github.com/StructFS/structfs/blob/main/isotope/spec/10-wasi-tower.md))
and its core-wasm binding
([spec 11](https://github.com/StructFS/structfs/blob/main/isotope/spec/11-core-wasm-binding.md));
it has zero idea what WIT is. This crate is one more way to get things
running as Isotope blocks — wasm components built with wit-bindgen or
wasip2 tooling — packaged as an `ArtifactLoader` the embedder registers:

```rust,ignore
let mut runtime = featherweight_runtime::Runtime::new();
featherweight_component::register(&mut runtime);
// component .wasm artifacts (layer 1) now load alongside core modules
```

## How it works

- The loader claims wasm **component** artifacts (layer field 1);
  core modules stay with the runtime's built-in binding.
- The WIT world (`wit/world.wit`) is a projection of the Block ABI, not
  its source of truth: an `ll-store` interface of two functions —
  `read(path) -> result<option<list<u8>>, string>` and
  `write(path, data) -> result<list<list<u8>>, string>` — where paths
  are byte-component sequences and data is raw bytes in the block's
  manifest-declared serialization (JSON, CBOR, or FlexBuffers). The WIT
  never changes when serialization formats do.
- The adapter wraps the block's namespace in a `CoreToLL` bridge with
  the declared codec, arms the same metering (fuel, epoch interruption)
  the core runtime applies, and calls the guest's `manifest()` and
  `run()` exports.

## Why adapter-tier

Keeping component-model machinery out of the core keeps the runtime's
wasmtime dependency minimal and keeps the ABI's story honest: an SDK
author targets two imports in the `structfs` module, and no host is
required to carry component tooling. Where wasip2 composition or
wit-bindgen language coverage is wanted, this adapter provides it —
one `register` call, no core changes.
