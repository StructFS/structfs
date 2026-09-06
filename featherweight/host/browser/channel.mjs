// A one-slot SharedArrayBuffer channel: main thread -> worker.
//
// This is what makes a *parked* mailbox read possible in a browser:
// the worker blocks in Atomics.wait inside the `structfs.read` import
// while the main thread stays responsive, exactly the
// parked-not-spinning distinction the runtime's Gate provides
// natively. One slot suffices — the producer keeps its own overflow
// queue and refills on the worker's "taken" notification.
//
// Layout: Int32Array [state (0 empty / 1 full), byte length], then the
// JSON payload bytes at offset 8.

const HEADER_BYTES = 8;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function createChannel(capacity = 64 * 1024) {
  return new SharedArrayBuffer(HEADER_BYTES + capacity);
}

/// Producer side (main thread). Never blocks: when the slot is full the
/// value waits in a local queue until the consumer signals "taken".
export class ChannelSender {
  constructor(sab) {
    this.header = new Int32Array(sab, 0, 2);
    this.body = new Uint8Array(sab, HEADER_BYTES);
    this.pending = [];
  }

  send(value) {
    const bytes = encoder.encode(JSON.stringify(value));
    if (bytes.length > this.body.length) {
      throw new Error(`message exceeds channel capacity: ${bytes.length}`);
    }
    this.pending.push(bytes);
    this.pump();
  }

  /// Move the next pending message into the slot if it is empty. Call
  /// on "taken" notifications from the consumer.
  pump() {
    if (this.pending.length === 0) return;
    if (Atomics.load(this.header, 0) !== 0) return;
    const bytes = this.pending.shift();
    this.body.set(bytes);
    this.header[1] = bytes.length;
    Atomics.store(this.header, 0, 1);
    Atomics.notify(this.header, 0);
  }
}

/// Consumer side (worker thread). `receive` parks until a message
/// arrives — only legal off the main thread.
export class ChannelReceiver {
  constructor(sab) {
    this.header = new Int32Array(sab, 0, 2);
    this.body = new Uint8Array(sab, HEADER_BYTES);
  }

  receive() {
    // wait returns "not-equal" immediately if a message is already
    // there — no lost-wakeup window.
    Atomics.wait(this.header, 0, 0);
    const bytes = this.body.slice(0, this.header[1]);
    Atomics.store(this.header, 0, 0);
    return JSON.parse(decoder.decode(bytes));
  }
}
