// Transcript tests: the store wrappers, the wire format's fidelity,
// worker-mode record/replay — and the cross-host half of the fixture
// contract: a transcript recorded by the native runtime replays here,
// against the same committed guest, byte-for-byte.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { Worker } from "node:worker_threads";

import {
  instantiate,
  RawJson,
  status,
  StoreError,
  valueText,
  type HostStore,
} from "../structfs-host.ts";
import {
  digestOf,
  fromJsonl,
  RecordingStore,
  ReplayingStore,
  toJsonl,
} from "../transcript.ts";
import { WorkerHost } from "../worker-host.ts";

/// A world of canned answers, for exercising the wrappers without a
/// guest: values by path, absences, and one refusal.
class CannedStore implements HostStore {
  read(path: string): unknown {
    if (path === "iso/random/uuid") return "u-1";
    if (path === "missing") return undefined;
    throw new StoreError(status.PERMISSION_DENIED, `not wired: ${path}`);
  }

  write(path: string, _value: unknown): string {
    return path;
  }
}

test("record then replay answers in order, refusals included", () => {
  const recording = new RecordingStore(new CannedStore());
  assert.equal(recording.read("iso/random/uuid"), "u-1");
  assert.equal(recording.read("missing"), undefined);
  assert.throws(() => recording.read("secrets"), StoreError);
  assert.equal(recording.write("iso/log/info", "ran"), "iso/log/info");

  const replay = new ReplayingStore(recording.entries);
  assert.equal(replay.read("iso/random/uuid"), "u-1");
  assert.equal(replay.read("missing"), undefined);
  assert.throws(
    () => replay.read("secrets"),
    (error: unknown) =>
      error instanceof StoreError &&
      error.code === status.PERMISSION_DENIED,
  );
  assert.equal(replay.write("iso/log/info", "ran"), "iso/log/info");
  assert.equal(replay.remaining(), 0);
});

test("divergence, different data, and exhaustion fail loudly", () => {
  const recording = new RecordingStore(new CannedStore());
  recording.read("iso/random/uuid");
  recording.write("iso/log/info", "ran");

  // A different question, with the entry index in the error.
  const asked = new ReplayingStore(recording.entries);
  assert.throws(
    () => asked.read("iso/time/now"),
    (error: unknown) =>
      error instanceof StoreError &&
      error.code === status.CONFLICT &&
      error.message.includes("diverged at entry 0"),
  );

  // The same path with different data is a diverged run.
  const lied = new ReplayingStore(recording.entries);
  lied.read("iso/random/uuid");
  assert.throws(
    () => lied.write("iso/log/info", "different"),
    (error: unknown) =>
      error instanceof StoreError &&
      error.message.includes("different data"),
  );

  // Asking more than the transcript holds runs it out.
  const out = new ReplayingStore(recording.entries);
  out.read("iso/random/uuid");
  out.write("iso/log/info", "ran");
  assert.throws(
    () => out.read("iso/random/uuid"),
    (error: unknown) =>
      error instanceof StoreError && error.message.includes("ran out"),
  );
});

test("the JSONL round trip preserves integers beyond 2^53", () => {
  const ns = "1788841819523489123";
  const recording = new RecordingStore({
    read: () => new RawJson(ns),
    write: (path) => path,
  });
  recording.read("iso/time/now_unix_ns");

  const entries = fromJsonl(toJsonl(recording.entries));
  const replay = new ReplayingStore(entries);
  const value = replay.read("iso/time/now_unix_ns");
  // Served back to a guest, the rendering is byte-identical — no
  // JSON.parse rounding anywhere on the path.
  assert.equal(valueText(value), ns);
});

test("digests are cross-host-comparable exactly when unambiguous", () => {
  // Matches the native runtime's fnv1a-64 over {"parsed":"hi"} — the
  // committed fixture pins the same value end to end.
  assert.equal(digestOf("hi"), "a528605174a7fbcb");
  assert.equal(digestOf(42), digestOf(42));
  assert.equal(digestOf({ nested: true }), undefined);
  assert.equal(digestOf(0.1), undefined);
});

test("cross-host: the native runtime's recording replays this guest", async () => {
  const wasm = await readFile(
    new URL("./fixtures/transcript-probe.wasm", import.meta.url),
  );
  const jsonl = await readFile(
    new URL("./fixtures/rust-recorded.jsonl", import.meta.url),
    "utf8",
  );
  const replay = new ReplayingStore(fromJsonl(jsonl));
  const guest = await instantiate(wasm, replay);
  assert.equal(guest.manifest().name, "transcript-probe");
  // Exit codes name the probe's first failed expectation; 0 means every
  // recorded answer — the uuid, the ns clock, the refusal, both write
  // acknowledgements and digests — crossed identically.
  assert.equal(guest.run(), 0);
  assert.equal(replay.remaining(), 0);
});

test("worker mode records and replays a resident run", async () => {
  const wasm = await readFile(new URL("../kv.wasm", import.meta.url));
  const recorded = await WorkerHost.start({
    createWorker: () => new Worker(new URL("../worker.ts", import.meta.url)),
    wasm,
    record: true,
  });
  await recorded.request("write", "greeting", "hello");
  const read = await recorded.request("read", "greeting");
  assert.deepEqual(read, { result: "ok", value: "hello" });
  assert.equal(await recorded.shutdown(), 0);

  const transcript = recorded.transcript();
  assert.ok(transcript !== undefined && transcript.length > 0);

  // Replay: the channel is never waited on, no requests are sent, and
  // the guest re-runs the recorded session to the same exit.
  const replayed = await WorkerHost.start({
    createWorker: () => new Worker(new URL("../worker.ts", import.meta.url)),
    wasm,
    replay: transcript,
  });
  assert.equal(await replayed.shutdown(), 0);
  assert.equal(replayed.transcriptRemaining(), 0);
});
