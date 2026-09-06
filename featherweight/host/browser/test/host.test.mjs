// Smoke tests for the JS host, against the real reference guest
// (featherweight-guest's wasm-kv). Build the guest first:
//
//   ./scripts/browser_host_test.sh
//
// which compiles kv.wasm into this directory's parent and runs these
// tests under `node --test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { Worker } from "node:worker_threads";

import { IsoStore } from "../iso-store.mjs";
import { instantiate, status, StoreError } from "../structfs-host.mjs";
import { WorkerHost } from "../worker-host.mjs";

const wasm = await readFile(new URL("../kv.wasm", import.meta.url)).catch(() => {
  throw new Error(
    "kv.wasm not found — run ./scripts/browser_host_test.sh to build it",
  );
});

test("batch mode: manifest and a kv round trip", async () => {
  const iso = new IsoStore();
  const written = iso.enqueue("write", "answer", { n: 42 });
  const read = iso.enqueue("read", "answer");
  const deleted = iso.enqueue("write", "answer"); // no data = delete
  const gone = iso.enqueue("read", "answer");

  const guest = await instantiate(wasm, iso);
  const manifest = guest.manifest();
  assert.equal(manifest.name, "wasm-kv");
  assert.equal(manifest.serialization, "application/json");

  // run() drains the queue; the empty queue reads null — the shutdown
  // signal — so the guest exits after serving.
  assert.equal(guest.run(), 0);

  assert.deepEqual(iso.responses.get(written), { result: "ok", path: "answer" });
  assert.deepEqual(iso.responses.get(read), { result: "ok", value: { n: 42 } });
  assert.deepEqual(iso.responses.get(gone), { result: "ok", value: null });
  assert.deepEqual(iso.responses.get(deleted), { result: "ok", path: "answer" });
  assert.deepEqual(iso.shutdownComplete, {});
  assert.equal(iso.interface.name, "wasm-kv");
});

test("unwired paths are denied, namespace-style", () => {
  const iso = new IsoStore();
  assert.throws(
    () => iso.read("iso/secrets"),
    (error) =>
      error instanceof StoreError && error.code === status.PERMISSION_DENIED,
  );
});

test("worker mode: a resident, stateful server", async () => {
  const host = await WorkerHost.start({
    createWorker: () =>
      new Worker(new URL("../worker.mjs", import.meta.url)),
    wasm,
  });
  assert.equal(host.manifest.name, "wasm-kv");

  const write = await host.request("write", "greeting", "hello");
  assert.deepEqual(write, { result: "ok", path: "greeting" });

  // The block stays parked on its mailbox between requests, so its
  // state survives — the property batch mode cannot offer.
  const first = await host.request("read", "greeting");
  assert.deepEqual(first, { result: "ok", value: "hello" });
  const second = await host.request("read", "greeting");
  assert.deepEqual(second, { result: "ok", value: "hello" });

  assert.equal(await host.shutdown(), 0);
});
