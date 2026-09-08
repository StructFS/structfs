# The browser host

A strict-TypeScript host for the core-wasm binding
(`isotope/spec/11-core-wasm-binding.md`): the same guest binaries the
native runtime executes with wasmtime run here, in a browser or in
Node, with no runtime dependencies. Two imports, `block_alloc`
delivery, the typed error taxonomy as status codes — the binding's
host side, demonstrated portable.

Transcripts (`isotope/spec/12-determinism.md`) are supported in the
same wire format the native runtime writes: a run recorded by `fw
run --record` replays here, and a run recorded here replays there.
The committed fixtures under `test/fixtures/` pin that contract in
both directions, digests included.

## Pieces

| File | What |
|------|------|
| `structfs-host.ts` | The binding host: `instantiate(wasmBytes, store)` → `{manifest, run}` |
| `iso-store.ts` | A minimal `/iso` surface: mailbox, responses, stdio, env/args, time, randomness, shutdown; everything else denied like an unwired namespace |
| `transcript.ts` | Spec 12 transcripts: `RecordingStore`/`ReplayingStore` wrapping any store, JSONL to/from the native runtime's format |
| `session.ts` | The session log (spec 12): an arrival-order forensic timeline across every block the page runs; `tap()` wraps any store transparently |
| `channel.ts` | One-slot SharedArrayBuffer channel — a parked mailbox read via `Atomics.wait` |
| `worker.ts` / `worker-host.ts` | Resident-server mode: the guest's synchronous `run()` lives in a worker, parked between requests; `record`/`replay` options wrap its store |
| `index.html` + `serve.ts` | The demo: wasm-kv resident in a worker, driven by buttons |
| `test/` | Node tests (`node --test`, TypeScript run natively) and the cross-host fixtures |

## Two drive modes

**Batch** (any thread, no SAB): queue requests on the `IsoStore`, call
`run()`. The drained mailbox reads `null` — the shutdown signal — so
the guest serves everything queued, writes `iso/shutdown/complete`,
and exits.

**Resident** (worker + SharedArrayBuffer): the mailbox read parks in
`Atomics.wait` while the main thread stays live. The block keeps its
state between requests, exactly as under the native runtime; responses
and stdio stream out via `postMessage`. Browsers require
cross-origin-isolation for SharedArrayBuffer, hence `serve.ts`'s
COOP/COEP headers.

## Transcripts

Wrap any store to record; replay needs no store at all:

```ts
import { RecordingStore, ReplayingStore, fromJsonl } from "./transcript.ts";

const recording = new RecordingStore(new IsoStore());
(await instantiate(wasm, recording)).run();
const jsonl = recording.jsonl();          // the native format

const replay = new ReplayingStore(fromJsonl(jsonl));
(await instantiate(wasm, replay)).run();  // the world never consulted
```

Resident mode takes `record: true` / `replay: jsonl` on
`WorkerHost.start`, plus `session: log, block: name` to witness every
operation into a shared `SessionLog` — several workers sharing one log
get a cross-block timeline, `seq` assigned in main-thread arrival
order. Under replay nothing parks — the channel is never waited on. Divergence (a different question, or the same write with
different data) fails loudly with the entry index; replay of a
transcript recorded by the native runtime preserves nanosecond
integers exactly (JSON.parse source access → `RawJson`).

## Run it

```bash
npm install          # dev-only: typescript
npm run check        # tsc --noEmit, strict
npm test             # node --test (Node ≥ 23.6 runs the .ts directly)
npm run build        # emit dist/ for the browser demo
node serve.ts        # then open http://localhost:8787

# Or everything, after building the kv guest:
../../../scripts/browser_host_test.sh
```

To regenerate the cross-host fixtures after changing the probe or the
wire format: `node test/record-fixture.ts` here, and
`cargo test -p featherweight-runtime --test transcript -- --ignored regenerate`
on the native side.
