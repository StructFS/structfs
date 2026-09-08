// The core-wasm binding, hosted in strict TypeScript
// (isotope/spec/11-core-wasm-binding.md).
//
// This is the whole host side of the Block ABI: two imports in the
// `structfs` module, result delivery through the guest's `block_alloc`,
// and the typed error taxonomy as negative status codes. No runtime
// dependencies, no tooling — the compiled module runs in a browser or
// in Node as-is, against the same guest binaries wasmtime runs.

export const status = {
  OK: 0,
  ABSENT: 1,
  NOT_FOUND: -1,
  PERMISSION_DENIED: -2,
  CONFLICT: -3,
  OVERLOADED: -4,
  DEADLINE_EXCEEDED: -5,
  CANCELLED: -6,
  INVALID_PATH: -7,
  RESOURCE_LIMIT: -8,
  OTHER: -9,
} as const;

export type Status = (typeof status)[keyof typeof status];

/// A store error carrying a spec 11 status code.
export class StoreError extends Error {
  readonly code: Status;

  constructor(code: Status, message: string) {
    super(message);
    this.code = code;
  }
}

/// A pre-serialized JSON payload: lets a store deliver values (like
/// nanosecond timestamps) that JSON.stringify would mangle.
export class RawJson {
  readonly text: string;

  constructor(text: string) {
    this.text = text;
  }
}

/// A JSON-shaped value as stores speak it. `RawJson` may stand in for
/// a value whose exact rendering matters; `undefined` from a read
/// means the path is absent.
export type StoreValue = unknown;

/// The store the binding host drives: paths as strings, JSON-shaped
/// values. Either operation may throw [`StoreError`] to cross a typed
/// error to the guest.
export interface HostStore {
  read(path: string): StoreValue | undefined;
  write(path: string, value: StoreValue): string;
}

/// Renders a store value as the JSON text the guest receives —
/// `RawJson` verbatim, everything else through JSON.stringify.
export function valueText(value: StoreValue): string {
  return value instanceof RawJson ? value.text : JSON.stringify(value);
}

interface GuestExports {
  memory: WebAssembly.Memory;
  block_alloc(len: number): number;
  manifest(retPtr: number): number;
  run(): number;
}

export interface GuestManifest {
  name?: string;
  serialization?: string;
  [key: string]: unknown;
}

export interface Guest {
  manifest(): GuestManifest;
  run(): number;
  exports: GuestExports;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/// Instantiate a core-binding guest over a store.
///
/// Both calls are synchronous, as the ABI is; a store whose mailbox
/// read parks (Atomics.wait) makes run() a resident server — see
/// worker.ts.
export async function instantiate(
  wasmBytes: BufferSource,
  store: HostStore,
): Promise<Guest> {
  let exports: GuestExports;

  // Views into guest memory are never cached: block_alloc may grow the
  // memory, detaching earlier ArrayBuffers.
  const guestBytes = (ptr: number, len: number): Uint8Array =>
    new Uint8Array(exports.memory.buffer, ptr, len).slice();

  // Deliver a payload per the binding: allocate via block_alloc, copy,
  // fill the ret record ({ptr, len}, little-endian u32s).
  const deliver = (retPtr: number, bytes: Uint8Array): void => {
    let ptr = 0;
    if (bytes.length > 0) {
      ptr = exports.block_alloc(bytes.length);
      new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
    }
    const view = new DataView(exports.memory.buffer);
    view.setUint32(retPtr, ptr, true);
    view.setUint32(retPtr + 4, bytes.length, true);
  };

  const fail = (retPtr: number, error: unknown): number => {
    const message =
      error instanceof Error ? error.message : String(error);
    deliver(retPtr, encoder.encode(message));
    return error instanceof StoreError ? error.code : status.OTHER;
  };

  const imports: WebAssembly.Imports = {
    structfs: {
      read(pathPtr: number, pathLen: number, retPtr: number): number {
        const path = decoder.decode(guestBytes(pathPtr, pathLen));
        try {
          const value = store.read(path);
          if (value === undefined) {
            deliver(retPtr, new Uint8Array(0));
            return status.ABSENT;
          }
          deliver(retPtr, encoder.encode(valueText(value)));
          return status.OK;
        } catch (error) {
          return fail(retPtr, error);
        }
      },
      write(
        pathPtr: number,
        pathLen: number,
        dataPtr: number,
        dataLen: number,
        retPtr: number,
      ): number {
        const path = decoder.decode(guestBytes(pathPtr, pathLen));
        const data = guestBytes(dataPtr, dataLen);
        try {
          const value: unknown = JSON.parse(decoder.decode(data));
          const resultPath = store.write(path, value);
          deliver(retPtr, encoder.encode(resultPath));
          return status.OK;
        } catch (error) {
          return fail(retPtr, error);
        }
      },
    },
  };

  const { instance } = await WebAssembly.instantiate(wasmBytes, imports);
  exports = instance.exports as unknown as GuestExports;

  /// Retrieve the guest's manifest (spec 01), parsed.
  const manifest = (): GuestManifest => {
    const retPtr = exports.block_alloc(8);
    const code = exports.manifest(retPtr);
    if (code !== status.OK) {
      throw new Error(`guest manifest returned status ${code}`);
    }
    const view = new DataView(exports.memory.buffer);
    const ptr = view.getUint32(retPtr, true);
    const len = view.getUint32(retPtr + 4, true);
    return JSON.parse(decoder.decode(guestBytes(ptr, len))) as GuestManifest;
  };

  /// The block's main; returns the guest's exit code.
  const run = (): number => exports.run();

  return { manifest, run, exports };
}
