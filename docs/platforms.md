# Platform and feature contracts

The 0.3 development line keeps portable schemas and cancellation independent of
native executor features. Native support is checked on Linux and macOS in release
CI. Browser compilation targets `wasm32-unknown-unknown`; a compile check does not
claim browser support for OS I/O or native Wasmtime execution.

| Surface | Native host | Browser-shared build | Selection |
| --- | --- | --- | --- |
| Core paths, values, patterns, wrappers | Yes | Yes | `structfs-core-store`; `async` enables both async trait families |
| Typed Serde and codecs | Yes | Yes | `structfs-serde-store`; `async` adds borrowing and detached helpers |
| Facade core/Serde/async | Yes | Yes | `structfs`, defaults disabled, `serde,async` |
| Cancellation, gates, streams, handle stores | Yes | Yes | `structfs-handles`, defaults disabled |
| SyncBridge | Yes | Not a browser blocking bridge | Handles `sync-bridge` (enabled by default) |
| HTTP request/response/status schemas | Yes | Yes | `structfs-http`, defaults disabled |
| HTTP stores and ReqwestExecutor | Yes | No | HTTP `blocking` (enabled by default); includes native executor |
| Featherweight guest SDK | Guest target | Yes | Existing guest feature combinations checked by the archive gate |
| Featherweight runtime and service cleanup example | Yes | No native-runtime support promised | Keep the selected Tokio runtime alive through engine ticking and retained cleanup |

Use target-specific dependencies for native host functionality in a shared crate:

```toml
[dependencies]
structfs-handles = { version = "0.3.0", default-features = false }
structfs-http = { version = "0.3.0", default-features = false }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
structfs-handles = { version = "0.3.0", features = ["sync-bridge"] }
structfs-http = { version = "0.3.0", features = ["blocking"] }
```

These versions describe the unreleased checkout. Cargo features are additive:
`default-features = false` on one dependency cannot undo a native feature enabled
elsewhere in the selected dependency graph. Library crates should request only
the features they use; executables select their executor. Tokio synchronization
primitives remain a dependency of portable handles, but native scheduling is not.

Independent consumer checks (also run against extracted Cargo archives):

```sh
cargo check --manifest-path tests/portable/Cargo.toml --locked --offline --target wasm32-unknown-unknown
cargo check --manifest-path tests/portable/Cargo.toml --locked --offline --features native
```

The browser host's own runtime tests remain in `featherweight/host/browser` and
run separately in CI. They establish behavior beyond Rust target compilation.
