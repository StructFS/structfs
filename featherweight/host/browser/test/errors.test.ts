// The error taxonomy is the native runtime's: this host's table must
// equal the committed error-kinds.json the native tests generate.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

import { errorKinds, labelOfStatus, statusOfLabel } from "../errors.ts";
import { IsoStore } from "../iso-store.ts";
import { instantiate, parsePath, status, StoreError } from "../structfs-host.ts";
import { ReplayingStore, RecordingStore } from "../transcript.ts";

test("the error table matches the native runtime's", async () => {
  const native = JSON.parse(
    await readFile(new URL("./fixtures/error-kinds.json", import.meta.url), "utf8"),
  ) as unknown;
  assert.deepEqual(errorKinds, native);
});

test("every status names its kind and every label its status", () => {
  for (const row of errorKinds) {
    if (row.canonical) assert.equal(labelOfStatus(row.status), row.label);
    if (row.kind !== "CodecResourceLimit") {
      assert.equal(statusOfLabel(row.label), row.status, row.label);
    }
  }
  assert.equal(statusOfLabel("codec", "resource_limit"), status.RESOURCE_LIMIT);
  assert.equal(statusOfLabel("unheard_of"), status.OTHER);
  assert.equal(labelOfStatus(-99), "other");
});

test("paths are validated and normalized as the native binding does", () => {
  assert.equal(parsePath("a//b/0/"), "a/b/0");
  assert.equal(parsePath("_x/données"), "_x/données");
  for (const bad of ["bad-path", "_", "a b", "a/-"]) {
    assert.throws(
      () => parsePath(bad),
      (error: unknown) =>
        error instanceof StoreError && error.code === status.INVALID_PATH,
      bad,
    );
  }
});

test("typed errors keep their kind and detail through a transcript", () => {
  const failing = (error: StoreError) => ({
    read(): never {
      throw error;
    },
    write(): never {
      throw error;
    },
  });
  const errors = [
    new StoreError(status.INVALID_ARGUMENT, "bad"),
    new StoreError(status.NOT_FOUND, "", { kind: "no_route", path: "x/y" }),
    new StoreError(status.INVALID_PATH, "bad component", {
      kind: "invalid_path",
      component: "a-b",
      position: 1,
    }),
    new StoreError(status.RESOURCE_LIMIT, "deep", {
      kind: "codec",
      codec: { kind: "resource_limit", format: "application/json" },
    }),
  ];
  for (const error of errors) {
    const recording = new RecordingStore(failing(error));
    assert.throws(() => recording.read("p"));
    const replay = new ReplayingStore(recording.entries);
    assert.throws(
      () => replay.read("p"),
      (replayed: unknown) =>
        replayed instanceof StoreError &&
        replayed.code === error.code &&
        JSON.stringify(replayed.detail) ===
          JSON.stringify({ kind: error.detail.kind ?? labelOfStatus(error.code), ...error.detail }),
    );
  }
});

test("an invalid guest path never reaches the store", async () => {
  const wasm = await readFile(new URL("./fixtures/transcript-probe.wasm", import.meta.url));
  const recording = new RecordingStore(new IsoStore());
  // The probe expects -7 for its invalid path, -8 and -10 for its
  // malformed iso reads; 0 means every expectation held.
  assert.equal((await instantiate(wasm, recording)).run(), 0);
  const paths = recording.entries.map((entry) => entry.path);
  assert.ok(!paths.some((path) => path.includes("bad")), JSON.stringify(paths));
  const kinds = recording.entries.flatMap((entry) =>
    typeof entry.answer === "object" && "failed" in entry.answer
      ? [entry.answer.failed.kind]
      : [],
  );
  assert.deepEqual(kinds, ["permission_denied", "resource_limit", "invalid_argument"]);
});
