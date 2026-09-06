// The core-wasm binding, hosted in JavaScript
// (isotope/spec/11-core-wasm-binding.md).
//
// This is the whole host side of the Block ABI: two imports in the
// `structfs` module, result delivery through the guest's `block_alloc`,
// and the typed error taxonomy as negative status codes. No
// dependencies, no tooling — it runs in a browser or in Node as-is,
// against the same guest binaries wasmtime runs.

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
};

/// A store error carrying a spec 11 status code.
export class StoreError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

/// A pre-serialized JSON payload: lets a store deliver values (like
/// nanosecond timestamps) that JSON.stringify would mangle.
export class RawJson {
  constructor(text) {
    this.text = text;
  }
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/// Instantiate a core-binding guest over a JS store.
///
/// The store speaks paths-as-strings and JSON-shaped values:
///   store.read(path)          -> value, or undefined when absent
///   store.write(path, value)  -> result path (a string)
/// Either may throw StoreError to cross a typed error to the guest.
///
/// Returns { manifest, run, exports }. Both calls are synchronous, as
/// the ABI is; a store whose mailbox read parks (Atomics.wait) makes
/// run() a resident server — see worker.mjs.
export async function instantiate(wasmBytes, store) {
  let exports;

  // Views into guest memory are never cached: block_alloc may grow the
  // memory, detaching earlier ArrayBuffers.
  const guestBytes = (ptr, len) =>
    new Uint8Array(exports.memory.buffer, ptr, len).slice();

  // Deliver a payload per the binding: allocate via block_alloc, copy,
  // fill the ret record ({ptr, len}, little-endian u32s).
  const deliver = (retPtr, bytes) => {
    let ptr = 0;
    if (bytes.length > 0) {
      ptr = exports.block_alloc(bytes.length);
      new Uint8Array(exports.memory.buffer, ptr, bytes.length).set(bytes);
    }
    const view = new DataView(exports.memory.buffer);
    view.setUint32(retPtr, ptr, true);
    view.setUint32(retPtr + 4, bytes.length, true);
  };

  const fail = (retPtr, error) => {
    deliver(retPtr, encoder.encode(String(error?.message ?? error)));
    return error instanceof StoreError ? error.code : status.OTHER;
  };

  const imports = {
    structfs: {
      read(pathPtr, pathLen, retPtr) {
        const path = decoder.decode(guestBytes(pathPtr, pathLen));
        try {
          const value = store.read(path);
          if (value === undefined) {
            deliver(retPtr, new Uint8Array(0));
            return status.ABSENT;
          }
          const text =
            value instanceof RawJson ? value.text : JSON.stringify(value);
          deliver(retPtr, encoder.encode(text));
          return status.OK;
        } catch (error) {
          return fail(retPtr, error);
        }
      },
      write(pathPtr, pathLen, dataPtr, dataLen, retPtr) {
        const path = decoder.decode(guestBytes(pathPtr, pathLen));
        const data = guestBytes(dataPtr, dataLen);
        try {
          const value = JSON.parse(decoder.decode(data));
          const resultPath = store.write(path, value);
          deliver(retPtr, encoder.encode(String(resultPath)));
          return status.OK;
        } catch (error) {
          return fail(retPtr, error);
        }
      },
    },
  };

  const { instance } = await WebAssembly.instantiate(wasmBytes, imports);
  exports = instance.exports;

  /// Retrieve the guest's manifest (spec 01), parsed.
  const manifest = () => {
    const retPtr = exports.block_alloc(8);
    const code = exports.manifest(retPtr);
    if (code !== status.OK) {
      throw new Error(`guest manifest returned status ${code}`);
    }
    const view = new DataView(exports.memory.buffer);
    const ptr = view.getUint32(retPtr, true);
    const len = view.getUint32(retPtr + 4, true);
    return JSON.parse(decoder.decode(guestBytes(ptr, len)));
  };

  /// The block's main; returns the guest's exit code.
  const run = () => exports.run();

  return { manifest, run, exports };
}
