/** Browser-compatible headless interactive v1 model. Rendering is application-owned.
 * Bigint sequences preserve the entire u64 domain; transport codecs must preserve it. */
export type Input = { type: "key" | "paste"; text: string } |
  { type: "resize"; columns: number; rows: number } |
  { type: "mouse"; x: number; y: number; button: number } | { type: "close" };
export interface InputEnvelope { version: number; session: string; sequence: bigint; input: Input }
const MAX = (1n << 64n) - 1n;
const utf8 = new TextEncoder();
function text(value: string): number {
  for (const c of value) { const n = c.codePointAt(0)!; if (n >= 0xd800 && n <= 0xdfff) throw Error("invalid Unicode"); }
  return utf8.encode(value).length;
}
export function validate(input: InputEnvelope, session: string, last: bigint): void {
  if (input.version !== 1 || input.session !== session || last < 0n || last >= MAX || input.sequence !== last + 1n) throw Error("invalid input identity or sequence");
  const exact = (value: object, keys: string[]) => { if (Object.keys(value).length !== keys.length || !keys.every(k => Object.hasOwn(value, k))) throw Error("unknown or missing fields"); };
  exact(input, ["version", "session", "sequence", "input"]);
  text(input.session);
  const p = input.input;
  const keys = { key: ["type", "text"], paste: ["type", "text"], resize: ["type", "columns", "rows"], mouse: ["type", "x", "y", "button"], close: ["type"] };
  if (!Object.hasOwn(keys, p.type)) throw Error("unknown input type");
  exact(p, keys[p.type]);
  if (p.type === "key" || p.type === "paste") text(p.text);
  if (p.type === "resize" && (![p.columns, p.rows].every(n => Number.isInteger(n) && n > 0 && n <= 0xffffffff))) throw Error("invalid resize");
  if (p.type === "mouse" && (![p.x, p.y].every(n => Number.isInteger(n) && n >= 0 && n <= 0xffffffff) || !Number.isInteger(p.button) || p.button < 0 || p.button > 255)) throw Error("invalid mouse");
}
export class HeadlessSession {
  accepted = 0n; processed = 0n; rendered: { epoch: string; revision: bigint } | undefined;
  closed = false;
  private events: InputEnvelope[] = [];
  private inFlight: InputEnvelope | undefined;
  private bytes = 0; private inputClosed = false;
  readonly id: string; readonly maxEvents: number; readonly maxBytes: number;
  constructor(id: string, maxEvents: number, maxBytes: number) {
    this.id = id; this.maxEvents = maxEvents; this.maxBytes = maxBytes;
    text(id);
    if (!Number.isSafeInteger(maxEvents) || maxEvents <= 0 || !Number.isSafeInteger(maxBytes) || maxBytes < 64) throw Error("invalid limits");
  }
  private weight(e: InputEnvelope): number { return 64 + text(e.session) + (e.input.type === "key" || e.input.type === "paste" ? text(e.input.text) : 0); }
  submit(e: InputEnvelope): void {
    if (this.closed || this.inputClosed) throw Error("closed"); validate(e, this.id, this.accepted);
    const weight = this.weight(e);
    if (this.events.length + Number(this.inFlight !== undefined) >= this.maxEvents || weight > this.maxBytes - this.bytes) throw Error("overloaded");
    // Retain a copy so a caller cannot mutate an accepted sequence/payload.
    this.events.push({ ...e, input: { ...e.input } }); this.bytes += weight; this.accepted = e.sequence; this.inputClosed = e.input.type === "close";
  }
  next(): InputEnvelope | undefined {
    if (this.closed) throw Error("closed"); if (this.inFlight) throw Error("unprocessed input");
    this.inFlight = this.events.shift(); return this.inFlight && { ...this.inFlight, input: { ...this.inFlight.input } };
  }
  acknowledge(sequence: bigint): void {
    if (this.inFlight?.sequence !== sequence) throw Error("wrong acknowledgment");
    this.bytes -= this.weight(this.inFlight); this.inFlight = undefined; this.processed = sequence;
  }
  present(token: { epoch: string; revision: bigint }): void {
    if (this.closed || text(token.epoch) > 128 || token.revision < 0n || token.revision > MAX || (this.rendered && (this.rendered.epoch !== token.epoch || this.rendered.revision > token.revision))) throw Error("invalid presentation");
    this.rendered = { ...token };
  }
  release(): void { this.closed = true; this.events = []; this.inFlight = undefined; this.bytes = 0; }
}
