// StructFS core-binding SDK for AssemblyScript
// (isotope/spec/11-core-wasm-binding.md).
//
// This file is the ENTIRE ABI surface — everything above it is ordinary
// library code. Compile with plain `asc` targeting wasm; export
// `manifest` and `run` from your block module alongside this SDK's
// `block_alloc`.
//
// Reference implementation: exercised against the spec's contract, not
// yet wired into this repo's CI (no AssemblyScript toolchain here).

@external("structfs", "read")
declare function structfs_read(p: usize, pl: i32, ret: usize): i32;
@external("structfs", "write")
declare function structfs_write(p: usize, pl: i32, d: usize, dl: i32, ret: usize): i32;

/** The guest's one obligation: hand the host writable memory. */
export function block_alloc(len: i32): usize {
  return heap.alloc(len);
}

function takeRet(ret: usize): Uint8Array {
  const ptr = load<u32>(ret);
  const len = load<u32>(ret + 4);
  const out = new Uint8Array(len as i32);
  if (len > 0) {
    memory.copy(out.dataStart, ptr, len);
    heap.free(ptr);
  }
  return out;
}

function message(ret: usize): string {
  const bytes = takeRet(ret);
  return String.UTF8.decodeUnsafe(bytes.dataStart, bytes.length);
}

/** StructFS read: the bytes, or null when absent. Throws on error. */
export function read(path: string): Uint8Array | null {
  const p = String.UTF8.encode(path);
  const ret = heap.alloc(8);
  const status = structfs_read(changetype<usize>(p), p.byteLength, ret);
  if (status == 0) {
    const out = takeRet(ret);
    heap.free(ret);
    return out;
  }
  if (status == 1) {
    heap.free(ret);
    return null;
  }
  const msg = message(ret);
  heap.free(ret);
  throw new Error(msg); // status carries the typed taxonomy (spec 11)
}

/** StructFS write: the result path. Throws on error. */
export function write(path: string, data: Uint8Array): string {
  const p = String.UTF8.encode(path);
  const ret = heap.alloc(8);
  const status = structfs_write(
    changetype<usize>(p), p.byteLength,
    data.dataStart, data.length,
    ret,
  );
  const result = status == 0 ? message(ret) : "";
  if (status != 0) {
    const msg = message(ret);
    heap.free(ret);
    throw new Error(msg);
  }
  heap.free(ret);
  return result;
}
