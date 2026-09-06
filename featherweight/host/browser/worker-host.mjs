// Main-thread side of the resident-server host: spawn the worker,
// stream requests in over the SAB channel, correlate responses by
// their respond_to ids.

import { ChannelSender, createChannel } from "./channel.mjs";

const listen = (worker, handler) => {
  if (typeof worker.on === "function") worker.on("message", handler);
  else worker.addEventListener("message", (event) => handler(event.data));
};

export class WorkerHost {
  /// Start a guest as a resident server.
  ///
  ///   createWorker — () => a Worker running worker.mjs (the caller
  ///                  owns construction: browser and Node differ)
  ///   wasm         — the guest module bytes (ArrayBuffer/TypedArray)
  ///   args, env    — the block's identity surface
  ///   onStdio      — (stream, text) for guest stdout/stderr
  ///   onLog        — (level, value) for iso/log writes
  ///
  /// Resolves once the guest's manifest crossed: the block is parked on
  /// its mailbox, serving.
  static start({ createWorker, wasm, args = [], env = {}, onStdio, onLog }) {
    const host = new WorkerHost();
    const sab = createChannel();
    host.sender = new ChannelSender(sab);
    host.waiters = new Map();
    host.nextId = 0;
    host.worker = createWorker();

    let started;
    const ready = new Promise((resolve, reject) => (started = { resolve, reject }));
    host.exited = new Promise((resolve) => (host.resolveExit = resolve));

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
          onStdio?.(message.stream, message.text);
          break;
        case "log":
          onLog?.(message.level, message.value);
          break;
        case "exit":
          host.resolveExit(message.code);
          break;
        case "error":
          started.reject(new Error(message.message));
          host.resolveExit(-1);
          break;
      }
    });

    host.worker.postMessage({ wasm, sab, args, env });
    return ready;
  }

  /// Send one server-protocol request; resolves with the block's
  /// response envelope ({result: "ok", value/path} or
  /// {result: "error", error}).
  request(op, path, data = null) {
    const id = this.nextId++;
    const envelope = { op, path, respond_to: `iso/server/responses/${id}` };
    if (data !== null) envelope.data = data;
    return new Promise((resolve) => {
      this.waiters.set(id, resolve);
      this.sender.send(envelope);
    });
  }

  /// Request shutdown (the null mailbox event); resolves with the
  /// guest's exit code.
  async shutdown() {
    this.sender.send(null);
    const code = await this.exited;
    this.worker.terminate?.();
    return code;
  }
}
