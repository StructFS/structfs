// Deterministic sources: the browser half of spec 12's determinism
// feature, bit-identical to the native runtime's.
//
// Same seed, same block, same run — on either host. The derivations are
// pinned, not merely similar: per-block state is `seed ^ fnv1a64(block
// key)`, the entropy stream is splitmix64 in little-endian words, uuids
// are sixteen seeded bytes with the RFC 4122 version/variant bits set,
// and the virtual clock starts at 2000-01-01T00:00:00Z advancing one
// millisecond per read. The committed seeded fixture pins all of it
// cross-host: a recording of a seeded run under the native runtime is
// byte-identical to one made here.
//
// Values that exceed 2^53 (the nanosecond clock, signed 64-bit ints)
// travel as RawJson so no JavaScript number ever rounds them.

import { RawJson } from "./structfs-host.ts";

const MASK64 = 0xffffffffffffffffn;

/// splitmix64, as the native runtime computes it.
const splitmix64 = (state: { value: bigint }): bigint => {
  state.value = (state.value + 0x9e3779b97f4a7c15n) & MASK64;
  let z = state.value;
  z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & MASK64;
  z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & MASK64;
  return (z ^ (z >> 31n)) & MASK64;
};

/// fnv1a-64 over UTF-8, as a bigint (the per-block seed derivation).
const fnv1a64 = (text: string): bigint => {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text)) {
    hash = ((hash ^ BigInt(byte)) * 0x100000001b3n) & MASK64;
  }
  return hash;
};

const EPOCH_NS = 946_684_800_000_000_000n; // 2000-01-01T00:00:00Z
const TICK_NS = 1_000_000n; // 1ms per read

/// Seeded time and entropy for one block: what `/iso/time` and
/// `/iso/random` answer under determinism.
export class SeededSources {
  private readonly entropy: { value: bigint };
  private ticks = 0n;

  /// The sources for `block` (its assembly-scoped transcript key) under
  /// `seed` — the same stream the native runtime derives.
  constructor(seed: bigint | number, block: string) {
    this.entropy = { value: (BigInt(seed) ^ fnv1a64(block)) & MASK64 };
  }

  /// The next `n` deterministic bytes.
  entropyBytes(n: number): number[] {
    const bytes: number[] = [];
    while (bytes.length < n) {
      let word = splitmix64(this.entropy);
      for (let i = 0; i < 8; i += 1) {
        bytes.push(Number(word & 0xffn));
        word >>= 8n;
      }
    }
    bytes.length = n;
    return bytes;
  }

  /// A seeded v4-shaped uuid: sixteen entropy bytes with the RFC 4122
  /// version and variant bits set, as the native runtime builds it.
  uuid(): string {
    const bytes = this.entropyBytes(16);
    bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
    bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
    const hex = bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
    return (
      `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-` +
      `${hex.slice(16, 20)}-${hex.slice(20)}`
    );
  }

  /// A seeded signed 64-bit integer (eight entropy bytes, little-endian),
  /// exact regardless of magnitude.
  int(): RawJson {
    const bytes = this.entropyBytes(8);
    let word = 0n;
    for (let i = 7; i >= 0; i -= 1) {
      word = (word << 8n) | BigInt(bytes[i] ?? 0);
    }
    return new RawJson(BigInt.asIntN(64, word).toString());
  }

  /// The virtual now in unix nanoseconds; every read advances the clock
  /// one tick, so time observably moves.
  nowUnixNs(): RawJson {
    const ns = EPOCH_NS + TICK_NS * this.ticks;
    this.ticks += 1n;
    return new RawJson(ns.toString());
  }

  /// The virtual now as ISO 8601 with millisecond precision — the
  /// format both hosts pin for seeded `time/now`.
  nowIso(): string {
    const ns = this.nowUnixNs();
    return new Date(Number(BigInt(ns.text) / 1_000_000n)).toISOString();
  }

  /// Virtual nanoseconds since block start; consumes one tick.
  monotonicNs(): RawJson {
    const ns = BigInt(this.nowUnixNs().text) - EPOCH_NS;
    return new RawJson(ns.toString());
  }

  /// `time/after/{ms}` under the virtual clock: the wait completes at
  /// once, having advanced virtual time by the requested span.
  advanceAfter(ms: number): void {
    this.ticks += (BigInt(ms) * 1_000_000n) / TICK_NS;
  }

  /// Fast-forward past a replayed prefix (spec 12 seek): `words`
  /// entropy draws and `ticks` clock reads.
  fastForward(words: number, ticks: number): void {
    for (let i = 0; i < words; i += 1) splitmix64(this.entropy);
    this.ticks += BigInt(ticks);
  }
}
