// Session log tests: the transparent tap, transcript linkage, and the
// cross-worker timeline — one log shared by two resident blocks, the
// main thread's arrival order as the witness.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { Worker } from "node:worker_threads";

import { IsoStore } from "../iso-store.ts";
import { SessionLog } from "../session.ts";
import { instantiate, status, StoreError } from "../structfs-host.ts";
import { RecordingStore } from "../transcript.ts";
import { WorkerHost } from "../worker-host.ts";

test("the tap is transparent and labels every outcome", () => {
  const session = new SessionLog();
  const iso = new IsoStore();
  const tapped = session.tap(iso, "demo/solo");

  const uuid = tapped.read("iso/random/uuid");
  assert.match(String(uuid), /^[0-9a-f-]{36}$/);
  assert.equal(tapped.write("iso/log/info", "ran"), "iso/log/info");
  assert.throws(
    () => tapped.read("secrets"),
    (error: unknown) =>
      error instanceof StoreError && error.code === status.PERMISSION_DENIED,
  );

  assert.deepEqual(
    session.entries.map((e) => [e.seq, e.block, e.op, e.path, e.outcome]),
    [
      [0, "demo/solo", "read", "iso/random/uuid", "found"],
      [1, "demo/solo", "write", "iso/log/info", "wrote"],
      [2, "demo/solo", "read", "secrets", "failed:permission_denied"],
    ],
  );
  // No transcript, no links — the flight recorder stands alone.
  assert.ok(session.entries.every((e) => e.entry === undefined));
  // The JSONL is the native format: one entry per line, seq dense.
  const lines = session.jsonl().trim().split("\n");
  assert.equal(lines.length, 3);
  assert.equal(JSON.parse(lines[2] ?? "").outcome, "failed:permission_denied");
});

test("recording links session entries into the transcript", async () => {
  const wasm = await readFile(
    new URL("./fixtures/transcript-probe.wasm", import.meta.url),
  );
  const session = new SessionLog();
  const recording = new RecordingStore(new IsoStore());
  const tapped = session.tap(recording, "cross-host/probe", recording);
  const guest = await instantiate(wasm, tapped);
  assert.equal(guest.run(), 0);

  // Five operations, each linked to its transcript index in order.
  assert.deepEqual(
    session.entries.map((e) => e.entry),
    [0, 1, 2, 3, 4],
  );
  // The join holds: the linked transcript entry is the same operation.
  for (const witnessed of session.entries) {
    const linked = recording.entries[witnessed.entry ?? -1];
    assert.equal(linked?.path, witnessed.path);
    assert.equal(linked?.op, witnessed.op);
  }
});

test("two resident blocks share one timeline", async () => {
  const wasm = await readFile(new URL("../kv.wasm", import.meta.url));
  const session = new SessionLog();
  const spawn = (block: string) =>
    WorkerHost.start({
      createWorker: () => new Worker(new URL("../worker.ts", import.meta.url)),
      wasm,
      session,
      block,
    });
  const left = await spawn("page/left");
  const right = await spawn("page/right");

  // Interleave requests across the two blocks.
  await left.request("write", "a", 1);
  await right.request("write", "b", 2);
  await left.request("read", "a");
  await right.request("read", "b");
  assert.equal(await left.shutdown(), 0);
  assert.equal(await right.shutdown(), 0);

  // One witness, one clock: seq is dense across both blocks, and both
  // appear — the assembly-wide timeline a per-block transcript cannot
  // give.
  assert.deepEqual(
    session.entries.map((e) => e.seq),
    session.entries.map((_, i) => i),
  );
  const blocks = new Set(session.entries.map((e) => e.block));
  assert.ok(blocks.has("page/left"), `${[...blocks]}`);
  assert.ok(blocks.has("page/right"), `${[...blocks]}`);
  // Each block's own line through the timeline preserves its order:
  // the kv guest reads its mailbox once per request served.
  const mailboxReads = session.entries.filter(
    (e) => e.block === "page/left" && e.path === "iso/server/requests",
  );
  assert.ok(mailboxReads.length >= 3, `${mailboxReads.length}`);
});
