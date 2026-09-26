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
let mut config = featherweight_runtime::RuntimeConfig::new(handle);
featherweight_component::register(&mut config);
let runtime = featherweight_runtime::Runtime::new(config);
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
- A loader shares one `ComponentEngine` (and one epoch ticker) across
  every component it loads. Each component is compiled and its
  `manifest()` inspected once (`ComponentEngine::prepare`); every run gets
  a fresh store.
- The adapter wraps the block's namespace in a `CoreToLL` bridge with
  the declared codec, arms the same metering the core binding applies
  (the per-run fuel cap from `Metering`, epoch interruption on
  cancellation), and calls the guest's `run()` export on a blocking
  worker. Errors are typed as in the core binding: engine setup failures
  are `RuntimeError::EngineConfig`, a panicked or cancelled worker is
  `HostPanic` or `ExecutionLost`, and instantiate or run failures are
  `RuntimeError::Wasm`.

## Why adapter-tier

Keeping component-model machinery out of the core keeps the runtime's
wasmtime dependency minimal and keeps the ABI's story honest: an SDK
author targets two imports in the `structfs` module, and no host is
required to carry component tooling. Where wasip2 composition or
wit-bindgen language coverage is wanted, this adapter provides it —
one `register` call, no core changes.

## Hosting support boundary

Components run only inside assemblies, through the loader; there is no
standalone run entry point. The native core-Wasm recoverable-host API
(`start_sync`/`start_async`) is not implemented by this adapter, and it does
not advertise host-state recovery or `ExecutionPolicy` support beyond the fuel
cap. Matching core import signatures is not a claim that native scheduling and
limit policies are available here. Pin SDK/runtime pairs to the documented
specification snapshot.
