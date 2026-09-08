// Worker entry: runs a core-binding guest as a resident server.
//
// The guest's synchronous run() lives here, off the main thread. Its
// mailbox read parks in Atomics.wait (ChannelReceiver) until the main
// thread sends a request; responses, stdio, and logs stream back via
// postMessage, which works even while run() holds the worker's event
// loop. A null request is the shutdown signal (spec 07): the guest
// drains, writes iso/shutdown/complete, and run() returns.
//
// Transcripts (spec 12) wrap the store here, at the boundary: `record`
// keeps every answer and ships the JSONL back with the exit message;
// `replay` answers every operation from supplied JSONL — the channel,
// the clock, and the entropy are then never consulted at all.
//
// Works both as a browser module worker and a Node worker_thread; the
// few lines below bridge the two message APIs.

import { ChannelReceiver } from "./channel.ts";
import { IsoStore } from "./iso-store.ts";
import { tapStore, type SessionEvent } from "./session.ts";
import { instantiate, type HostStore } from "./structfs-host.ts";
import { fromJsonl, RecordingStore, ReplayingStore } from "./transcript.ts";

export interface WorkerInit {
  wasm: BufferSource;
  sab: SharedArrayBuffer;
  args: string[];
  env: Record<string, string>;
  /// Keep a transcript of the run; it arrives on the exit message.
  record?: boolean;
  /// Replay from this JSONL instead of running live.
  replay?: string;
  /// Witness every boundary operation to the session log under this
  /// block name; events stream to the main thread, which assigns
  /// arrival order.
  session?: string;
}

export type WorkerMessage =
  | { type: "ready"; manifest: unknown }
  | { type: "taken" }
  | { type: "response"; id: number; value: unknown }
  | { type: "stdio"; stream: string; text: string }
  | { type: "session"; event: SessionEvent }
  | { type: "log"; level: string; value: unknown }
  | {
      type: "exit";
      code: number;
      shutdownComplete: unknown;
      transcript?: string;
      transcriptRemaining?: number;
    }
  | { type: "error"; message: string };

const workerScope = typeof self !== "undefined" ? self : undefined;
const nodePort = workerScope
  ? null
  : (await import("node:worker_threads")).parentPort;

const post = (message: WorkerMessage): void => {
  if (workerScope) workerScope.postMessage(message);
  else nodePort?.postMessage(message);
};

const onMessage = (handler: (init: WorkerInit) => void): void => {
  if (workerScope) {
    workerScope.onmessage = (event) => handler(event.data as WorkerInit);
  } else nodePort?.on("message", handler);
};

onMessage(async ({ wasm, sab, args, env, record, replay, session }) => {
  try {
    const channel = new ChannelReceiver(sab);
    const iso = new IsoStore({
      args,
      env,
      mailbox: () => {
        const value = channel.receive();
        post({ type: "taken" });
        return value;
      },
      onResponse: (id, value) => post({ type: "response", id, value }),
      onStdio: (stream, text) => post({ type: "stdio", stream, text }),
      onLog: (level, value) => post({ type: "log", level, value }),
    });

    // Under replay nothing parks: every answer — the mailbox included —
    // comes from the transcript, so the channel is never waited on.
    let store: HostStore = iso;
    let recording: RecordingStore | undefined;
    let replaying: ReplayingStore | undefined;
    if (replay !== undefined) {
      replaying = new ReplayingStore(fromJsonl(replay));
      store = replaying;
    } else if (record === true) {
      recording = new RecordingStore(iso);
      store = recording;
    }
    if (session !== undefined) {
      // The session tap observes the boundary as the guest sees it —
      // outside the transcript wrapper, linked into it when one exists.
      store = tapStore(
        store,
        session,
        (event) => post({ type: "session", event }),
        recording ?? replaying,
      );
    }

    const guest = await instantiate(wasm, store);
    const manifest = guest.manifest();
    const serialization = manifest.serialization ?? "application/json";
    if (serialization !== "application/json") {
      throw new Error(
        `this host speaks application/json; the guest declares ${serialization}`,
      );
    }
    post({ type: "ready", manifest });
    const code = guest.run();
    const exit: WorkerMessage = {
      type: "exit",
      code,
      shutdownComplete: iso.shutdownComplete,
    };
    if (recording !== undefined) exit.transcript = recording.jsonl();
    if (replaying !== undefined) {
      exit.transcriptRemaining = replaying.remaining();
    }
    post(exit);
  } catch (error) {
    post({
      type: "error",
      message: error instanceof Error ? error.message : String(error),
    });
  }
});
