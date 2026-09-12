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
