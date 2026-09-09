// Determinism tests: the seeded sources' pinned derivations, and the
// flagship — the same seed is the same run on both hosts, held to the
// committed fixture the native runtime recorded.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import { SeededSources } from "../determinism.ts";
import { IsoStore } from "../iso-store.ts";
import { instantiate, status, StoreError, valueText } from "../structfs-host.ts";
import {
  fromJsonl,
  RecordingStore,
  toJsonl,
  type TranscriptEntry,
} from "../transcript.ts";

test("seeded sources are stable per block and distinct across blocks", () => {
  const a1 = new SeededSources(42, "demo/api").entropyBytes(16);
  const a2 = new SeededSources(42, "demo/api").entropyBytes(16);
  const b = new SeededSources(42, "demo/cache").entropyBytes(16);
  assert.deepEqual(a1, a2);
  assert.notDeepEqual(a1, b);
});

test("the virtual clock starts at the epoch and moves one tick per read", () => {
  const sources = new SeededSources(7, "demo/api");
  assert.equal(sources.nowUnixNs().text, "946684800000000000");
  assert.equal(sources.nowUnixNs().text, "946684800001000000");
  sources.advanceAfter(30_000);
  assert.equal(sources.nowUnixNs().text, "946684830002000000");
});

test("seeded uuids carry the version and variant bits", () => {
  const uuid = new SeededSources(42, "demo/api").uuid();
  assert.match(
    uuid,
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
});

test("unknown iso paths are absent; non-iso paths are denied", () => {
  const iso = new IsoStore();
  // The parity rule both hosts pin: absent within the mounted iso
  // surface, unwired-namespace denial outside it.
  assert.equal(iso.read("iso/secrets"), undefined);
  assert.throws(
    () => iso.read("secrets"),
    (error: unknown) =>
      error instanceof StoreError && error.code === status.PERMISSION_DENIED,
  );
});

test("cross-host: the same seed is the same run on both hosts", async () => {
  const wasm = await readFile(
    new URL("./fixtures/transcript-probe.wasm", import.meta.url),
  );
  const iso = new IsoStore({
    sources: new SeededSources(42, "cross-host/probe"),
  });
  const recording = new RecordingStore(iso);
  assert.equal((await instantiate(wasm, recording)).run(), 0);

  const committed = fromJsonl(
    await readFile(
      new URL("./fixtures/seeded-recorded.jsonl", import.meta.url),
      "utf8",
    ),
  );
  // Error messages are host diagnostics, not contract — the kind is the
  // branchable part (spec 12); everything else must match exactly:
  // the seeded uuid, the virtual timestamp, acknowledgements, digests.
  const contract = (entries: readonly TranscriptEntry[]): string =>
    toJsonl(
      entries.map((entry) =>
        typeof entry.answer === "object" && "failed" in entry.answer
          ? {
              ...entry,
              answer: {
                failed: { ...entry.answer.failed, message: "" },
              },
            }
          : entry,
      ),
    );
  assert.equal(contract(recording.entries), contract(committed));
  // Spot-check the two seeded values against the committed run.
  assert.equal(
    valueText(new SeededSources(42, "cross-host/probe").uuid()),
    '"a536d69f-7aef-4ce7-a22f-6ceb48463b4d"',
  );
});
