// A minimal /iso surface for browser-hosted blocks
// (isotope/spec/04-system-paths.md, 07-server-protocol.md).
//
// Serves what a resident server block needs: the mailbox
// (iso/server/requests), response paths (iso/server/responses/{n}),
// identity (self/args, env), stdio, logs, time, randomness, and
// shutdown/complete. Everything else is denied, exactly as an unwired
// namespace path would be.

import {
  RawJson,
  status,
  StoreError,
  type HostStore,
  type StoreValue,
} from "./structfs-host.ts";

export interface ServerEnvelope {
  op: string;
  path: string;
  respond_to: string;
  data?: unknown;
}

export interface IsoStoreOptions {
  /// The block's identity surface.
  args?: string[];
  env?: Record<string, string>;
  /// Overrides the built-in queue; () => request value | null (null is
  /// the shutdown signal).
  mailbox?: () => unknown;
  /// Called for iso/server/responses/{id} writes.
  onResponse?: (id: number, value: unknown) => void;
  /// Called for stdout/stderr writes.
  onStdio?: (stream: string, text: string) => void;
  /// Called for iso/log/{level} writes.
  onLog?: (level: string, value: unknown) => void;
}

export class IsoStore implements HostStore {
  readonly args: string[];
  readonly env: Record<string, string>;
  readonly responses = new Map<number, unknown>();
  interface: unknown = undefined;
  shutdownComplete: unknown = undefined;

  private readonly mailbox: (() => unknown) | undefined;
  private readonly onResponse: IsoStoreOptions["onResponse"];
  private readonly onStdio: IsoStoreOptions["onStdio"];
  private readonly onLog: IsoStoreOptions["onLog"];
  private readonly queue: ServerEnvelope[] = [];
  private nextResponse = 0;

  constructor(options: IsoStoreOptions = {}) {
    this.args = options.args ?? [];
    this.env = options.env ?? {};
    this.mailbox = options.mailbox;
    this.onResponse = options.onResponse;
    this.onStdio = options.onStdio;
    this.onLog = options.onLog;
  }

  /// Queue a server-protocol request (batch mode): mints the
  /// respond_to path and returns the response id to collect afterward.
  enqueue(op: string, path: string, data: unknown = null): number {
    const id = this.nextResponse++;
    const envelope: ServerEnvelope = {
      op,
      path,
      respond_to: `iso/server/responses/${id}`,
    };
    if (data !== null) envelope.data = data;
    this.queue.push(envelope);
    return id;
  }

  read(path: string): StoreValue | undefined {
    const randomBytes = path.match(/^iso\/random\/bytes\/(\d+)$/);
    if (randomBytes !== null) {
      const n = Number(randomBytes[1]);
      if (n > 1 << 20) {
        throw new StoreError(
          status.RESOURCE_LIMIT,
          "random/bytes limited to 1MiB",
        );
      }
      const bytes = new Uint8Array(n);
      crypto.getRandomValues(bytes);
      return Array.from(bytes);
    }
    switch (path) {
      case "iso/server/requests":
        // The built-in queue serves batch mode: drain, then null — the
        // shutdown signal — so the guest exits cleanly when done.
        return this.mailbox ? this.mailbox() : (this.queue.shift() ?? null);
      case "iso/self/args":
        return this.args;
      case "iso/env":
        return this.env;
      case "iso/time/now":
        return new Date().toISOString();
      case "iso/time/now_unix_ns":
        // Nanoseconds exceed 2^53; RawJson keeps the integer exact.
        return new RawJson((BigInt(Date.now()) * 1000000n).toString());
      case "iso/time/monotonic":
        return Math.round(performance.now() * 1e6);
      case "iso/time/zone":
        return "UTC";
      case "iso/random/uuid":
        return crypto.randomUUID();
      case "iso/random/int": {
        const words = new Uint32Array(2);
        crypto.getRandomValues(words);
        // A safe integer, so the JSON rendering is exact.
        return ((words[0] ?? 0) % 0x200000) * 0x100000000 + (words[1] ?? 0);
      }
      default:
        throw new StoreError(status.PERMISSION_DENIED, `not wired: ${path}`);
    }
  }

  write(path: string, value: StoreValue): string {
    const response = path.match(/^iso\/server\/responses\/(\d+)$/);
    if (response !== null) {
      const id = Number(response[1]);
      this.responses.set(id, value);
      this.onResponse?.(id, value);
      return path;
    }
    const stdio = path.match(/^iso\/stdio\/(stdout|stderr)$/);
    if (stdio !== null && stdio[1] !== undefined) {
      this.onStdio?.(stdio[1], String(value));
      return path;
    }
    const log = path.match(/^iso\/log\/(\w+)$/);
    if (log !== null && log[1] !== undefined) {
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
