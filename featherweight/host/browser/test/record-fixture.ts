// Regenerates the committed js-recorded.jsonl fixture: a live run of
// the transcript probe under this host, recorded. The native runtime's
// tests replay it — the other half of the cross-host contract. Run by
// hand when the probe or the wire format changes:
//
//   node test/record-fixture.ts

import { readFile, writeFile } from "node:fs/promises";

import { IsoStore } from "../iso-store.ts";
import { instantiate } from "../structfs-host.ts";
import { RecordingStore } from "../transcript.ts";

const wasm = await readFile(
  new URL("./fixtures/transcript-probe.wasm", import.meta.url),
);
const recording = new RecordingStore(new IsoStore());
const guest = await instantiate(wasm, recording);
const code = guest.run();
if (code !== 0) {
  throw new Error(`the probe failed its expectation #${code} on a live run`);
}
const out = new URL("./fixtures/js-recorded.jsonl", import.meta.url);
await writeFile(out, recording.jsonl());
console.log(`recorded ${recording.entries.length} entries to ${out.pathname}`);
