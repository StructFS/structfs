// Main-thread side of the resident-server host: spawn the worker,
// stream requests in over the SAB channel, correlate responses by
// their respond_to ids.

import { ChannelSender, createChannel } from "./channel.ts";
import type { SessionLog } from "./session.ts";
import type { WorkerInit, WorkerMessage } from "./worker.ts";

/// The subset of Worker both a browser Worker and a Node worker_thread
/// satisfy, as this host uses it.
export interface WorkerLike {
  postMessage(message: unknown): void;
  terminate?(): unknown;
  on?(event: "message", handler: (message: WorkerMessage) => void): void;
  addEventListener?(
    event: "message",
    handler: (event: { data: WorkerMessage }) => void,
  ): void;
}

export interface WorkerHostOptions {
  /// () => a Worker running worker.ts (the caller owns construction:
  /// browser and Node differ).
  createWorker: () => WorkerLike;
  /// The guest module bytes.
  wasm: BufferSource | ArrayBufferLike;
  /// The block's identity surface.
  args?: string[];
  env?: Record<string, string>;
  onStdio?: (stream: string, text: string) => void;
  onLog?: (level: string, value: unknown) => void;
  /// Transcripts (spec 12), mixable with everything else: `record`
  /// keeps the run's boundary answers (read them from `transcript()`
  /// after shutdown); `replay` runs the guest from recorded JSONL with
  /// the live world never consulted.
  record?: boolean;
  replay?: string;
  /// With `replay`: seek — replay this many entries (`true` = all),
  /// then hand off to live execution; the guest keeps its replayed
  /// state and serves live requests from the channel.
  seek?: number | true;
  /// Session forensics (spec 12): every boundary operation the guest
  /// makes is witnessed into this log under `block`. Share one log
  /// across several hosts and the main thread's arrival order is the
  /// cross-worker timeline.
  session?: SessionLog;
  /// The block name session entries are witnessed under.
  block?: string;
  /// Deterministic time and entropy (spec 12), derived from the seed
  /// and the block name — mix freely with everything else.
  determinism?: { seed: number; block: string };
}

export type ResponseEnvelope =
  | { result: "ok"; value?: unknown; path?: string }
  | { result: "error"; error: unknown };

const listen = (
  worker: WorkerLike,
  handler: (message: WorkerMessage) => void,
): void => {
  if (typeof worker.on === "function") worker.on("message", handler);
  else worker.addEventListener?.("message", (event) => handler(event.data));
};

interface Exit {
  code: number;
  transcript?: string;
  transcriptRemaining?: number;
}

export class WorkerHost {
  manifest: unknown;

  private sender!: ChannelSender;
  private waiters!: Map<number, (value: unknown) => void>;
  private nextId!: number;
  private worker!: WorkerLike;
  private exited!: Promise<Exit>;
  private resolveExit!: (exit: Exit) => void;
  private exit: Exit | undefined;

  /// Start a guest as a resident server. Resolves once the guest's
  /// manifest crossed: the block is parked on its mailbox, serving.
  static start(options: WorkerHostOptions): Promise<WorkerHost> {
    const host = new WorkerHost();
    const sab = createChannel();
    host.sender = new ChannelSender(sab);
    host.waiters = new Map();
    host.nextId = 0;
    host.worker = options.createWorker();

    let started: {
      resolve: (host: WorkerHost) => void;
      reject: (error: Error) => void;
    };
    const ready = new Promise<WorkerHost>((resolve, reject) => {
      started = { resolve, reject };
    });
    host.exited = new Promise<Exit>((resolve) => {
      host.resolveExit = resolve;
    });

    listen(host.worker, (message) => {
      switch (message.type) {
        case "ready":
          host.manifest = message.manifest;
          started.resolve(host);
          break;
        case "taken":
          host.sender.pump();
          break;
        case "response": {
          const waiter = host.waiters.get(message.id);
          host.waiters.delete(message.id);
          waiter?.(message.value);
          break;
        }
        case "stdio":
          options.onStdio?.(message.stream, message.text);
          break;
        case "session":
          options.session?.witness(message.event);
          break;
        case "log":
          options.onLog?.(message.level, message.value);
          break;
        case "exit": {
          const exit: Exit = { code: message.code };
          if (message.transcript !== undefined) {
            exit.transcript = message.transcript;
          }
          if (message.transcriptRemaining !== undefined) {
            exit.transcriptRemaining = message.transcriptRemaining;
          }
          host.exit = exit;
          host.resolveExit(exit);
          break;
        }
        case "error":
          started.reject(new Error(message.message));
          host.resolveExit({ code: -1 });
          break;
      }
    });

    const init: WorkerInit = {
      wasm: options.wasm as BufferSource,
      sab,
      args: options.args ?? [],
      env: options.env ?? {},
    };
    if (options.record !== undefined) init.record = options.record;
    if (options.replay !== undefined) init.replay = options.replay;
    if (options.seek !== undefined) init.seek = options.seek;
    if (options.session !== undefined) init.session = options.block ?? "block";
    if (options.determinism !== undefined) init.determinism = options.determinism;
    host.worker.postMessage(init);
    return ready;
  }

  /// Send one server-protocol request; resolves with the block's
  /// response envelope.
  request(op: string, path: string, data: unknown = null): Promise<unknown> {
    const id = this.nextId++;
    const envelope: Record<string, unknown> = {
      op,
      path,
      respond_to: `iso/server/responses/${id}`,
    };
    if (data !== null) envelope["data"] = data;
    return new Promise((resolve) => {
      this.waiters.set(id, resolve);
      this.sender.send(envelope);
    });
  }

  /// Request shutdown (the null mailbox event); resolves with the
  /// guest's exit code.
  async shutdown(): Promise<number> {
    this.sender.send(null);
    const exit = await this.exited;
    this.worker.terminate?.();
    return exit.code;
  }

  /// The recorded run's transcript as JSONL — present after shutdown
  /// of a run started with `record: true`.
  transcript(): string | undefined {
    return this.exit?.transcript;
  }

  /// Entries a replayed run left unconsumed — present after shutdown
  /// of a run started with `replay`.
  transcriptRemaining(): number | undefined {
    return this.exit?.transcriptRemaining;
  }
}
