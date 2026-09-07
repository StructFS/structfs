# featherweight-runtime

A strawman [Isotope](https://github.com/StructFS/structfs/tree/main/isotope/spec)
runtime: blocks are pico-processes whose entire world is
[StructFS](https://github.com/StructFS/structfs) reads and writes.

- **Blocks** run native Rust (`NativeBlock`) or core-binding wasm
  (`CoreWasmBlock`) against a per-block **namespace**: unwired paths are
  denied, reads and writes alike. Filesystem or network access is
  granted by wiring, never ambient.
- **`/iso/`** is the syscall surface — identity, env, time, randomness,
  stdio, logging, timers, shutdown — served as ordinary store paths.
- **The server protocol** makes every block a store: operations routed
  to a block become `{op, path, data, respond_to}` requests read from
  `iso/server/requests`; the block's response write resolves the
  caller's parked operation.
- **Assemblies** compose blocks with capability wiring; nested assembly
  definitions instantiate recursively, the public block starts eagerly,
  everything else lazily on first access.
- **Metering** governs guest execution: optional fuel caps and epoch
  interruption, so immediate shutdown can stop a spinning guest while
  parked store reads stay untouched.

## The wasm binding

The runtime core speaks exactly one wasm binding — the
[core-wasm binding](https://github.com/StructFS/structfs/blob/main/isotope/spec/11-core-wasm-binding.md):
two imports in the `structfs` module, no bindgen or component tooling.
Plain `cargo build --target wasm32-unknown-unknown` output runs
directly.

Other artifact kinds attach as adapters through
`Runtime::register_loader` — for example `featherweight-component`
teaches the runtime to run WIT component-model artifacts as blocks.
WASI is a shim above the Block ABI (`featherweight-wasi`), never a
runtime dependency.

## Example

```rust,no_run
use std::collections::HashMap;
use featherweight_runtime::{register_builtins, AssemblyDef, Runtime};

# async fn demo() -> featherweight_runtime::Result<()> {
let mut runtime = Runtime::new();
register_builtins(&mut runtime);

let def = AssemblyDef::from_str(
    r#"{"assembly": "demo", "blocks": {"kv": "builtin:kv"}, "public": "kv"}"#,
)?;
let assembly = runtime.instantiate(&def, HashMap::new(), ".".as_ref())?;

use structfs_core_store::{path, Value};
assembly.write(path!("users/alice"), Value::from("hi")).await?;
assert_eq!(
    assembly.read(path!("users/alice")).await?,
    Some(Value::from("hi")),
);
# Ok(()) }
```

## Status

This is the reference strawman for the Isotope spec, not a production
OS: JSON/CBOR/FlexBuffers transports are supported at the block
boundary, but there is no hash verification, no registries, no restart
policy, and no deadlock detection. The
[spec](https://github.com/StructFS/structfs/tree/main/isotope/spec) is
the contract; this crate is the working model of it.
