# featherweight-guest

The Rust guest SDK for the Isotope core-wasm binding
([spec 11](https://github.com/StructFS/structfs/blob/main/isotope/spec/11-core-wasm-binding.md)),
plus the reference guest: a kv store served over the Isotope server
protocol.

The core-wasm binding is deliberately small — the entire ABI surface a
guest touches is the `sdk` module:

- two imports from the `structfs` module (`read` and `write` over
  linear memory),
- one export the guest provides (`block_alloc`, a bump allocator
  suffices),
- safe wrappers: `structfs_read(path) -> Result<Option<Vec<u8>>, String>`
  and `structfs_write(path, data) -> Result<String, String>`.

Payload bytes are in the serialization the guest's `manifest()`
declares (plain JSON, tagged StructFS Value JSON v1, CBOR, or FlexBuffers on the
reference runtime).

Enable the optional `value-codecs` feature for `sdk::read_value` and
`sdk::write_value`. Pass a `structfs_serde_store::ValueCodec` with the profile and
limits your host supports, and declare its format in your manifest. For example,
`Profile::ValueJson` uses `application/vnd.structfs.value+json;version=1` and
preserves bytes, full u64 values, and non-finite floats. The feature builds for
`wasm32-unknown-unknown`; it does not change the reference guest's JSON contract.

## Building a block

No componentization, no bindgen:

```bash
cargo build --target wasm32-unknown-unknown --release
```

The resulting `.wasm` runs directly under
[featherweight-runtime](https://crates.io/crates/featherweight-runtime),
under the JavaScript browser host, or under any host implementing the
binding's two imports.

A minimal block:

```rust,ignore
use featherweight_guest::sdk;

#[no_mangle]
pub unsafe extern "C" fn manifest(ret: *mut sdk::Ret) -> i32 {
    // fill ret with your JSON manifest bytes; see the crate source
    0
}

#[no_mangle]
pub extern "C" fn run() -> i32 {
    // read requests from "iso/server/requests", serve, respond;
    // write "iso/shutdown/complete" when done
    0
}
```

The crate's own `lib.rs` is the worked example: the wasm-kv block reads
its mailbox, serves read/write requests, and exits cleanly on the
shutdown signal — about a hundred lines, most of them ordinary library
code.

The optional `state` feature adds `state::Client`, using the selected Value codec
and the portable `structfs-state` protocol for batches, snapshots, and observation.
State faults remain typed reply values. The host owns handles across guest traps;
release handles explicitly when done. Batches never retry automatically.

The optional `profiles` feature adds `sdk::profiles(root, codec)` to read a
granted provider's `meta/profiles`, plus portable input, operation, approval,
configuration-acknowledgment and process-request schemas. It implies
`value-codecs` and adds no imports. Discovery does not invoke the provider's
effectful start/read operations. Profile declarations describe support; they do
not grant authority.

The reference server emits explicit read-response presence. Hosts and servers
using the legacy Null-as-absence envelope should migrate using Isotope 07's
`present` field. The sample kv store retains its documented delete-on-null
convention; the Value and ABI layers distinguish present Null from absence.

`sdk::read_typed` and `sdk::write_typed` preserve `HostError { status, message }`.
Value/profile helpers use that typed host error; existing `structfs_read` and
`structfs_write` remain diagnostic-only compatibility wrappers.
