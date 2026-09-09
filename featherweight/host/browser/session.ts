// The session log: assembly-wide forensics for the browser host
// (isotope/spec/12-determinism.md), in the native runtime's format.
//
// One arrival-order witness of every boundary operation across every
// block this page runs: `{seq, block, op, path, outcome, entry?}` per
// line, `entry` linking into the block's transcript when one is kept
// or consumed. Observation-class: the log answers nothing, replay
// never reads it, and appending never fails the operation observed.
//
// For blocks resident in workers, entries stream to the main thread as
// they happen and the session log assigns `seq` on arrival — the main
// thread is the witness, and cross-worker order is the order it saw,
// exactly the honest claim a shared capture handle can make.

import { StoreError, type HostStore, type StoreValue } from "./structfs-host.ts";

/// One boundary operation, as the session witnessed it.
export interface SessionEntry {
  seq: number;
  block: string;
  op: "read" | "write";
  path: string;
  /// `found`, `absent`, `wrote`, or `failed:<kind>`.
  outcome: string;
  entry?: number;
}

/// A witnessed operation before the session assigns its place.
export type SessionEvent = Omit<SessionEntry, "seq">;

const kindOf = (error: unknown): string => {
  if (!(error instanceof StoreError)) return "other";
  switch (error.code) {
    case -1:
      return "not_found";
    case -2:
      return "permission_denied";
    case -3:
      return "conflict";
    case -4:
      return "overloaded";
    case -5:
      return "deadline_exceeded";
    case -6:
      return "cancelled";
    case -8:
      return "resource_limit";
    default:
      return "other";
  }
};

/// A transcript-shaped position source: both RecordingStore and
/// ReplayingStore satisfy it.
export interface Positioned {
  position(): number;
  /// A seek that reached its horizon: live operations link to nothing.
  handedOff?(): boolean;
}

export class SessionLog {
  readonly entries: SessionEntry[] = [];

  /// Witness one event, assigning its arrival order.
  witness(event: SessionEvent): void {
    this.entries.push({ seq: this.entries.length, ...event });
  }

  /// The log as JSONL, one entry per line — the native format.
  jsonl(): string {
    return this.entries.length === 0
      ? ""
      : `${this.entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`;
  }

  /// Wrap a store so every operation through it is witnessed under
  /// `block`. See [`tapStore`].
  tap(store: HostStore, block: string, positioned?: Positioned): HostStore {
    return tapStore(store, block, (event) => this.witness(event), positioned);
  }
}

/// Wrap a store so every operation through it is reported to `witness`
/// — transparent, Inspector-style: the store's answers pass through
/// unchanged, and witnessing never fails an operation. `positioned` (a
/// Recording/ReplayingStore) links events into the block's transcript.
/// A worker taps with a forwarding witness and the main thread's
/// [`SessionLog`] assigns arrival order.
export function tapStore(
  store: HostStore,
  block: string,
  witness: (event: SessionEvent) => void,
  positioned?: Positioned,
): HostStore {
  const observe = (event: SessionEvent): void => {
    try {
      witness(event);
    } catch {
      // Forensics must never change the run it observes.
    }
  };
  const at = (): { entry?: number } =>
    positioned === undefined || positioned.handedOff?.() === true
      ? {}
      : { entry: positioned.position() };
  return {
    read(path: string): StoreValue | undefined {
      const link = at();
      try {
        const value = store.read(path);
        observe({
          block,
          op: "read",
          path,
          outcome: value === undefined ? "absent" : "found",
          ...link,
        });
        return value;
      } catch (error) {
        observe({
          block,
          op: "read",
          path,
          outcome: `failed:${kindOf(error)}`,
          ...link,
        });
        throw error;
      }
    },
    write(path: string, value: StoreValue): string {
      const link = at();
      try {
        const result = store.write(path, value);
        observe({ block, op: "write", path, outcome: "wrote", ...link });
        return result;
      } catch (error) {
        observe({
          block,
          op: "write",
          path,
          outcome: `failed:${kindOf(error)}`,
          ...link,
        });
        throw error;
      }
    },
  };
}
