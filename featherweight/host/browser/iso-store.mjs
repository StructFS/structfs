// A minimal /iso surface for browser-hosted blocks
// (isotope/spec/04-the-iso-store.md, 07-server-protocol.md).
//
// Serves what a resident server block needs: the mailbox
// (iso/server/requests), response paths (iso/server/responses/{n}),
// identity (self/args, env), stdio, logs, time, and
// shutdown/complete. Everything else is denied, exactly as an unwired
// namespace path would be.

import { RawJson, StoreError, status } from "./structfs-host.mjs";

export class IsoStore {
  /// Options:
  ///   args, env      — the block's identity surface
  ///   mailbox        — () => request value | null; overrides the
  ///                    built-in queue (null is the shutdown signal)
  ///   onResponse     — (id, value) called for iso/server/responses/{id}
  ///   onStdio        — (stream, text) for stdout/stderr writes
  ///   onLog          — (level, value) for iso/log/{level} writes
  constructor({ args = [], env = {}, mailbox, onResponse, onStdio, onLog } = {}) {
    this.args = args;
    this.env = env;
    this.mailbox = mailbox;
    this.onResponse = onResponse;
    this.onStdio = onStdio;
    this.onLog = onLog;

    this.queue = [];
    this.nextResponse = 0;
    this.responses = new Map();
    this.interface = undefined;
    this.shutdownComplete = undefined;
  }

  /// Queue a server-protocol request (batch mode): mints the
  /// respond_to path and returns the response id to collect afterward.
  enqueue(op, path, data = null) {
    const id = this.nextResponse++;
    const envelope = { op, path, respond_to: `iso/server/responses/${id}` };
    if (data !== null) envelope.data = data;
    this.queue.push(envelope);
    return id;
  }

  read(path) {
    switch (path) {
      case "iso/server/requests":
        // The built-in queue serves batch mode: drain, then null — the
        // shutdown signal — so the guest exits cleanly when done.
        return this.mailbox ? this.mailbox() : (this.queue.shift() ?? null);
      case "iso/self/args":
        return this.args;
      case "iso/env":
        return this.env;
      case "iso/time/now_unix_ns":
        // Nanoseconds exceed 2^53; RawJson keeps the integer exact.
        return new RawJson((BigInt(Date.now()) * 1000000n).toString());
      case "iso/time/monotonic":
        return Math.round(performance.now() * 1e6);
      default:
        throw new StoreError(status.PERMISSION_DENIED, `not wired: ${path}`);
    }
  }

  write(path, value) {
    const response = path.match(/^iso\/server\/responses\/(\d+)$/);
    if (response) {
      const id = Number(response[1]);
      this.responses.set(id, value);
      this.onResponse?.(id, value);
      return path;
    }
    const stdio = path.match(/^iso\/stdio\/(stdout|stderr)$/);
    if (stdio) {
      this.onStdio?.(stdio[1], String(value));
      return path;
    }
    const log = path.match(/^iso\/log\/(\w+)$/);
    if (log) {
      this.onLog?.(log[1], value);
      return path;
    }
    switch (path) {
      case "iso/self/interface":
        this.interface = value;
        return path;
      case "iso/shutdown/complete":
        this.shutdownComplete = value;
        return path;
      default:
        throw new StoreError(status.PERMISSION_DENIED, `not wired: ${path}`);
    }
  }
}
