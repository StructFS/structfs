# Platform and feature contracts

The published 0.4.0 release and the 0.5 development line (this checkout;
unreleased, and its crates still carry version 0.4.0 until release) both keep
portable schemas and cancellation independent of native executor features.
The table's "Selection" column names the 0.5 crates and facade features; the
"0.4.0 (published)" column gives what differs in the published release.
Native support is checked on Linux and macOS in release CI. Browser
compilation targets `wasm32-unknown-unknown`; a compile check does not claim
browser support for OS I/O or native Wasmtime execution.

| Surface | Native host | Browser-shared build | Selection (0.5 development line) | 0.4.0 (published) |
| --- | --- | --- | --- | --- |
| Core paths, values, patterns, wrappers | Yes | Yes | `structfs-core-store`; `async` enables both async trait families | Same |
| Typed Serde and codecs | Yes | Yes | `structfs-serde-store`; `async` adds borrowing and detached helpers | Same |
| Facade core/typed/async | Yes | Yes | `structfs`, defaults disabled, `typed,async` | Facade feature `serde` instead of `typed` |
| Cancellation, gates, streams, handle stores | Yes | Yes | `structfs-handles` (no optional features), or facade `handles` | `structfs-handles` with `default-features = false` (its default `sync-bridge` feature is native); no facade feature |
| HTTP request/response/status schemas | Yes | Yes | `structfs-http`, defaults disabled, or facade `net` | `structfs-http`, defaults disabled; the facade `http` feature enables `blocking` |
| HTTP stores and BlockingReqwestExecutor | Yes | No | HTTP `blocking` (enabled by default), or facade `net-blocking`; includes native executor | HTTP `blocking`, or facade `http`; the executor is `ReqwestExecutor` |
| HTTP streaming executor | Yes | No | HTTP `streaming`, or facade `net-streaming` | HTTP `streaming`, or facade `http-streaming` |
| Featherweight guest SDK | Guest target | Yes | Existing guest feature combinations checked by the archive gate |
| Featherweight runtime and service cleanup example | Yes | No native-runtime support promised | Keep the selected Tokio runtime alive through engine ticking and retained cleanup |

Use target-specific dependencies for native host functionality in a shared crate:

```toml
[dependencies]
structfs-handles = { version = "0.4.0", default-features = false }
structfs-http = { version = "0.4.0", default-features = false }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
# 0.4 only: structfs-handles also offered features = ["sync-bridge"] here.
# From 0.5 the handles crate has no optional features.
structfs-http = { version = "0.4.0", features = ["blocking"] }
```

These versions describe the published 0.4.0 line. Cargo features are additive:
`default-features = false` on one dependency cannot undo a native feature enabled
elsewhere in the selected dependency graph. Library crates should request only
the features they use; executables select their executor. Tokio synchronization
primitives remain a dependency of portable handles, but native scheduling is not.

On the 0.5 development line the same split is reachable through the facade
alone. `0.5.0` below is the version these crates will carry when released; in
this checkout they are still numbered 0.4.0 and consumed by path:

```toml
[dependencies]
structfs = { version = "0.5.0", default-features = false, features = ["typed", "async", "net", "handles"] }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
structfs = { version = "0.5.0", features = ["net-blocking"] }
```

`tests/portable` consumes exactly this profile (its `native` feature enables
`structfs/net-blocking`).

Independent consumer checks (also run against extracted Cargo archives):

```sh
cargo check --manifest-path tests/portable/Cargo.toml --locked --offline --target wasm32-unknown-unknown
cargo check --manifest-path tests/portable/Cargo.toml --locked --offline --features native
```

The browser host's own runtime tests remain in `featherweight/host/browser` and
run separately in CI. They establish behavior beyond Rust target compilation.
