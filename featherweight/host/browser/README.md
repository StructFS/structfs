# The browser host

A JavaScript host for the core-wasm binding
(`isotope/spec/11-core-wasm-binding.md`): the same guest binaries the
native runtime executes with wasmtime run here, in a browser or in
Node, against ~120 lines of dependency-free ES module. Two imports,
`block_alloc` delivery, the typed error taxonomy as status codes — the
binding's host side, demonstrated portable.

## Pieces

| File | What |
|------|------|
| `structfs-host.mjs` | The binding host: `instantiate(wasmBytes, store)` → `{manifest, run}` |
| `iso-store.mjs` | A minimal `/iso` surface: mailbox, responses, stdio, env/args, time, shutdown; everything else denied like an unwired namespace |
| `channel.mjs` | One-slot SharedArrayBuffer channel — a parked mailbox read via `Atomics.wait` |
| `worker.mjs` / `worker-host.mjs` | Resident-server mode: the guest's synchronous `run()` lives in a worker, parked between requests |
| `index.html` + `serve.mjs` | The demo: wasm-kv resident in a worker, driven by buttons |
| `test/host.test.mjs` | Node smoke tests against the real kv guest |

## Two drive modes

**Batch** (any thread, no SAB): queue requests on the `IsoStore`, call
`run()`. The drained mailbox reads `null` — the shutdown signal — so
the guest serves everything queued, writes `iso/shutdown/complete`,
and exits.

**Resident** (worker + SharedArrayBuffer): the mailbox read parks in
`Atomics.wait` while the main thread stays live. The block keeps its
state between requests, exactly as under the native runtime; responses
and stdio stream out via `postMessage`. Browsers require
cross-origin-isolation for SharedArrayBuffer, hence `serve.mjs`'s
COOP/COEP headers.

## Run it

```bash
# Tests (builds kv.wasm from featherweight-guest, then node --test):
./scripts/browser_host_test.sh

# Demo:
node featherweight/host/browser/serve.mjs   # then open localhost:8787
```

Needs Node 20+ and the `wasm32-unknown-unknown` target. Not wired into
`quality_gates.sh`, which stays hermetic to the Rust toolchain.

## The store interface

The host is generic over a JS store, mirroring `Reader`/`Writer`:

```js
store.read(path)         // -> value, or undefined when absent
store.write(path, value) // -> result path (a string)
// either may throw StoreError(status.X, message)
```

Paths cross as strings, payloads as JSON (the manifest's declared
serialization). `RawJson` wraps pre-serialized text for values
JavaScript numbers would mangle (nanosecond timestamps).
