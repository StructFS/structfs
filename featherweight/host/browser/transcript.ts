// Per-block transcripts for the browser host
// (isotope/spec/12-determinism.md).
//
// The same wire format the native runtime writes: one JSON entry per
// line, `{op, path, answer, wrote?}`, answers as the four kinds —
// `{"found": {"parsed": value}}`, `"absent"`, `{"wrote": path}`,
// `{"failed": {kind, message, path?}}`. A run recorded by `fw` replays
// here, and a run recorded here replays under the native runtime;
// the cross-host fixtures in test/fixtures pin that byte-for-byte.
//
// The transcript rules are spec 12's: a refusal is an answer, write
// acknowledgements are inputs, the path (and here the payload digest)
// rides beside the answer as the divergence check, and replay fails
// loudly — with the entry index — at the first divergence.
//
// Numeric fidelity: found values that are integers beyond 2^53 (the
// nanosecond clock) are preserved through JSON.parse source access and
// served to the guest as RawJson, byte-for-byte. Digests are computed
// only for payloads whose rendering is unambiguous across hosts
// (strings, booleans, null, safe integers, RawJson); for anything else
// the digest is omitted, and replay compares digests only when both
// sides have one — degraded gracefully rather than falsely divergent.

import {
  RawJson,
  status,
  StoreError,
  valueText,
  type HostStore,
  type Status,
  type StoreValue,
} from "./structfs-host.ts";

/// An error as the transcript holds it (the native runtime's shape).
export interface TranscriptError {
  kind: string;
  message: string;
  path?: string;
}

/// The native runtime's Record envelope: parsed values or raw bytes.
export type RecordRepr =
  | { parsed: unknown }
  | { raw: { bytes: number[]; format: unknown } };

/// What the world said, externally tagged as serde renders it.
export type TranscriptAnswer =
  | { found: RecordRepr }
  | "absent"
  | { wrote: string }
  | { failed: TranscriptError };

/// One boundary operation and its complete answer.
export interface TranscriptEntry {
  op: "read" | "write";
  path: string;
  answer: TranscriptAnswer;
  wrote?: string;
}

const statusOf: Record<string, Status> = {
  not_found: status.NOT_FOUND,
  no_route: status.NOT_FOUND,
  permission_denied: status.PERMISSION_DENIED,
  conflict: status.CONFLICT,
  overloaded: status.OVERLOADED,
  deadline_exceeded: status.DEADLINE_EXCEEDED,
  resource_limit: status.RESOURCE_LIMIT,
  cancelled: status.CANCELLED,
};

const kindOf = (code: Status): string => {
  switch (code) {
    case status.NOT_FOUND:
      return "not_found";
    case status.PERMISSION_DENIED:
      return "permission_denied";
    case status.CONFLICT:
      return "conflict";
    case status.OVERLOADED:
      return "overloaded";
    case status.DEADLINE_EXCEEDED:
      return "deadline_exceeded";
    case status.RESOURCE_LIMIT:
      return "resource_limit";
    case status.CANCELLED:
      return "cancelled";
    default:
      return "other";
  }
};

const toTranscriptError = (error: unknown): TranscriptError => {
  if (error instanceof StoreError) {
    return { kind: kindOf(error.code), message: error.message };
  }
  const message = error instanceof Error ? error.message : String(error);
  return { kind: "other", message };
};

const toStoreError = (failed: TranscriptError): StoreError => {
  const code = statusOf[failed.kind] ?? status.OTHER;
  // Typed errors carry their path where the message would be empty
  // (the native runtime renders NotFound that way).
  const message =
    failed.message !== "" ? failed.message : (failed.path ?? failed.kind);
  return new StoreError(code, message);
};

/// fnv1a-64 over UTF-8, hex — the digest the native runtime computes.
export function fnv1a64(text: string): string {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text)) {
    hash ^= BigInt(byte);
    hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return hash.toString(16).padStart(16, "0");
}

/// Whether a payload's JSON rendering is unambiguous across hosts —
/// the precondition for a cross-host-comparable digest.
const unambiguous = (value: StoreValue): boolean =>
  value instanceof RawJson ||
  value === null ||
  typeof value === "string" ||
  typeof value === "boolean" ||
  (typeof value === "number" && Number.isSafeInteger(value));

/// The write-payload digest, over the Record envelope the native
/// runtime hashes; undefined when the rendering could differ by host.
export function digestOf(value: StoreValue): string | undefined {
  if (!unambiguous(value)) return undefined;
  return fnv1a64(`{"parsed":${valueText(value)}}`);
}

/// The found-answer envelope for a recorded value, as JSON text —
/// RawJson spliced verbatim so exact renderings survive.
const foundText = (value: StoreValue): string =>
  `{"found":{"parsed":${valueText(value)}}}`;

/// Serialize entries to the JSONL the native runtime reads and writes.
export function toJsonl(entries: readonly TranscriptEntry[]): string {
  return entries
    .map((entry) => {
      const pieces: string[] = [
        `"op":${JSON.stringify(entry.op)}`,
        `"path":${JSON.stringify(entry.path)}`,
      ];
      const found =
        typeof entry.answer === "object" && "found" in entry.answer
          ? entry.answer.found
          : undefined;
      if (found !== undefined && "parsed" in found) {
        pieces.push(`"answer":${foundText(found.parsed)}`);
      } else {
        pieces.push(`"answer":${JSON.stringify(entry.answer)}`);
      }
      if (entry.wrote !== undefined) {
        pieces.push(`"wrote":${JSON.stringify(entry.wrote)}`);
      }
      return `{${pieces.join(",")}}`;
    })
    .join("\n");
}

/// Parse JSONL entries, preserving exact renderings of found values
/// that JSON.parse would mangle (integers beyond 2^53) as RawJson.
export function fromJsonl(text: string): TranscriptEntry[] {
  const entries: TranscriptEntry[] = [];
  for (const [index, line] of text.split("\n").entries()) {
    if (line.trim() === "") continue;
    let entry: TranscriptEntry;
    try {
      entry = JSON.parse(line, function (key, value: unknown, context?: { source?: string }) {
        if (
          key === "parsed" &&
          typeof value === "number" &&
          context?.source !== undefined &&
          context.source !== String(value)
        ) {
          return new RawJson(context.source);
        }
        return value;
      }) as TranscriptEntry;
    } catch (error) {
      throw new Error(
        `transcript line ${index + 1} is not a JSON entry: ${String(error)}`,
      );
    }
    entries.push(entry);
  }
  return entries;
}

/// Execute live, keeping every boundary answer — refusals included.
/// A store wrapper: same interface in, transcript out.
export class RecordingStore implements HostStore {
  readonly entries: TranscriptEntry[] = [];
  private readonly inner: HostStore;

  constructor(inner: HostStore) {
    this.inner = inner;
  }

  jsonl(): string {
    return this.entries.length === 0 ? "" : `${toJsonl(this.entries)}\n`;
  }

  read(path: string): StoreValue | undefined {
    try {
      const value = this.inner.read(path);
      this.entries.push({
        op: "read",
        path,
        answer:
          value === undefined
            ? "absent"
            : { found: { parsed: value } },
      });
      return value;
    } catch (error) {
      this.entries.push({
        op: "read",
        path,
        answer: { failed: toTranscriptError(error) },
      });
      throw error;
    }
  }

  write(path: string, value: StoreValue): string {
    const wrote = digestOf(value);
    try {
      const resultPath = this.inner.write(path, value);
      const entry: TranscriptEntry = {
        op: "write",
        path,
        answer: { wrote: resultPath },
      };
      if (wrote !== undefined) entry.wrote = wrote;
      this.entries.push(entry);
      return resultPath;
    } catch (error) {
      const entry: TranscriptEntry = {
        op: "write",
        path,
        answer: { failed: toTranscriptError(error) },
      };
      if (wrote !== undefined) entry.wrote = wrote;
      this.entries.push(entry);
      throw error;
    }
  }
}

/// Answer every operation from the transcript. The inner store does
/// not exist: the transcript is the world, and a question the recorded
/// run never asked — or a write with different data — is divergence,
/// reported loudly with the entry index.
export class ReplayingStore implements HostStore {
  private cursor = 0;
  private readonly entries: readonly TranscriptEntry[];

  constructor(entries: readonly TranscriptEntry[]) {
    this.entries = entries;
  }

  /// Entries the replay has not consumed — a replayed run that exits
  /// with entries remaining stopped short of the recorded one.
  remaining(): number {
    return this.entries.length - this.cursor;
  }

  private next(op: "read" | "write", path: string): TranscriptEntry {
    const index = this.cursor;
    const entry = this.entries[index];
    if (entry === undefined) {
      throw new StoreError(
        status.CONFLICT,
        `the transcript ran out at entry ${index}, ${op} ${path} — the ` +
          `replayed run asked more of the world than the recorded one did`,
      );
    }
    this.cursor += 1;
    if (entry.op !== op || entry.path !== path) {
      throw new StoreError(
        status.CONFLICT,
        `replay diverged at entry ${index}: the transcript recorded ` +
          `${entry.op} ${entry.path}, the run asked ${op} ${path} — the ` +
          `block was not a function of its inputs`,
      );
    }
    return entry;
  }

  read(path: string): StoreValue | undefined {
    const { answer } = this.next("read", path);
    if (answer === "absent") return undefined;
    if ("found" in answer) {
      const repr = answer.found;
      if ("parsed" in repr) return repr.parsed;
      const { bytes, format } = repr.raw;
      if (format === "json" || format === "JSON") {
        return new RawJson(new TextDecoder().decode(new Uint8Array(bytes)));
      }
      throw new StoreError(
        status.OTHER,
        `the transcript holds raw ${String(format)} bytes this host cannot serve`,
      );
    }
    if ("failed" in answer) throw toStoreError(answer.failed);
    throw new StoreError(
      status.CONFLICT,
      `the transcript holds a write acknowledgement where the run read ${path}`,
    );
  }

  write(path: string, value: StoreValue): string {
    const entry = this.next("write", path);
    const actual = digestOf(value);
    if (entry.wrote !== undefined && actual !== undefined && entry.wrote !== actual) {
      throw new StoreError(
        status.CONFLICT,
        `replay diverged at write ${path}: the run wrote different data ` +
          `than the recorded run did (digest ${actual}, recorded ${entry.wrote})`,
      );
    }
    const { answer } = entry;
    if (typeof answer === "object" && "wrote" in answer) return answer.wrote;
    if (typeof answer === "object" && "failed" in answer) {
      throw toStoreError(answer.failed);
    }
    throw new StoreError(
      status.CONFLICT,
      `the transcript holds a read answer where the run wrote ${path}`,
    );
  }
}
