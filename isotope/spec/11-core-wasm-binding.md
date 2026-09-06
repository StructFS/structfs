# The Core-Wasm Binding

The core-wasm binding projects the Block ABI (spec 10) onto plain
WebAssembly core modules. It is the **SDK binding**: implementable by
hand in any language that targets wasm — AssemblyScript, TinyGo, C,
Zig, Rust, hand-written wat — in a few dozen lines, with no bindgen, no
component tooling, and no canonical-ABI machinery. It is also the
binding a browser host or any non-component runtime implements.

## Imports

The host provides exactly two functions, in the import module
**`structfs`** (they are the StructFS interface; `/iso` is merely a
path prefix inside the namespace they address):

```wat
(import "structfs" "read"
  (func (param $path_ptr i32) (param $path_len i32)
        (param $ret_ptr i32) (result i32)))          ;; status

(import "structfs" "write"
  (func (param $path_ptr i32) (param $path_len i32)
        (param $data_ptr i32) (param $data_len i32)
        (param $ret_ptr i32) (result i32)))          ;; status
```

## Exports

The guest provides:

```wat
(memory (export "memory") ...)
(func (export "block_alloc") (param $len i32) (result i32))  ;; -> ptr
(func (export "manifest") (param $ret_ptr i32) (result i32)) ;; status
(func (export "run") (result i32))                           ;; exit code
```

- `block_alloc(len)` returns a pointer to `len` bytes of guest memory
  the host may write into during the current call. It is the guest's
  single obligation beyond the entry points; a bump allocator suffices.
- `manifest` fills the ret record with the JSON manifest bytes
  (spec 01) and returns status 0.
- `run` is the block's main; its return value is the exit code used
  only if the block did not write `iso/shutdown/complete` (which takes
  precedence, as for every binding).

## The ret record

`ret_ptr` names 8 bytes of guest memory, 4-aligned, little-endian:

```
offset 0: u32 ptr    offset 4: u32 len
```

The host fills it by calling `block_alloc(len)`, copying the payload,
and storing `{ptr, len}`. Ownership of the buffer passes to the guest;
the host retains no pointers after the call returns. What the payload
is depends on the status:

| status | ret payload |
|---|---|
| `0` (read) | the data bytes, in the block's declared serialization |
| `0` (write) | the result path, UTF-8, in the caller's namespace |
| `0` (manifest) | the manifest JSON bytes |
| `1` (absent) | none — ret is zeroed |
| `< 0` (error) | a UTF-8 diagnostic message |

## Status codes

`0` success, `1` absent (reads only). Errors are negative, carrying the
protocol error taxonomy (spec 06) — the typed contract survives this
binding natively:

| status | meaning | errno analogue |
|---|---|---|
| `-1` | not found (operations requiring existence) | `ENOENT` |
| `-2` | permission denied / not wired | `ENOTCAPABLE` |
| `-3` | conflict | `EEXIST` |
| `-4` | overloaded / unavailable | `EAGAIN` |
| `-5` | deadline exceeded | `ETIMEDOUT` |
| `-6` | cancelled (interrupted parked read) | `EINTR` |
| `-7` | invalid path | `EINVAL` |
| `-8` | resource limit | — |
| `-9` | other store/codec error | `EIO` |

## Path encoding

Paths cross as plain UTF-8 joined strings (`services/kv/users/alice`).
This is lossless **because of the path grammar**: components are UAX#31
identifiers or numerics and can never contain `/`, so joining is
unambiguous. The host validates on entry; a malformed path is status
`-7`. (Raw LL byte components outside the validated grammar are not
representable in this binding; nothing above the LL layer produces
them.)

## Rules

1. **Stateless calls.** Every call is complete in itself. Hosts MUST
   NOT keep per-call state between calls (no pending-result protocols,
   no call ordering). Guests may call in any order, from any point in
   `run`.
2. **No re-execution.** The host performs each operation exactly once —
   result delivery uses `block_alloc`, never a retry-with-larger-buffer
   protocol, because reads are effectful (a mailbox read pops an
   event).
3. **Blocking is real.** `read` may park the calling (block) thread per
   the store's contract; `/iso/meta` declares which paths do.
4. **Reentrancy is limited to `block_alloc`.** During a host call, the
   host may invoke only `block_alloc`; it MUST NOT call other guest
   exports.
5. **Versioning.** This is `structfs` binding v1. Incompatible
   revisions use a new import module name; the two-function surface is
   expected to be stable.

## Relation to the other bindings

Identical semantics to the native trait binding and the component
binding (spec 10 lists all four). This is the one wasm binding a
runtime core implements; other bindings attach as artifact-loader
adapters that recognize their own artifact kind (a component's layer
field distinguishes it) — the core never learns their tooling. The
component binding remains preferable where wasip2 composition or
wit-bindgen coverage is wanted; this binding is preferable everywhere
an SDK author starts from scratch — which is the case this binding
exists for.

Reference hosts: `featherweight/runtime/src/core_wasm.rs` (wasmtime)
and `featherweight/host/browser` (dependency-free JavaScript, browser
and Node) — the same guest binaries run under both, which is the
host-neutrality this binding claims, demonstrated.

## Reference SDK sketch (AssemblyScript)

```ts
@external("structfs", "read")
declare function structfs_read(p: usize, pl: i32, ret: usize): i32;
@external("structfs", "write")
declare function structfs_write(p: usize, pl: i32, d: usize, dl: i32, ret: usize): i32;

export function block_alloc(len: i32): usize { return heap.alloc(len); }

function takeRet(ret: usize): Uint8Array {
  const ptr = load<u32>(ret), len = load<u32>(ret + 4);
  const out = new Uint8Array(len as i32);
  memory.copy(out.dataStart, ptr, len);
  heap.free(ptr);
  return out;
}

export function read(path: string): Uint8Array | null {
  const p = String.UTF8.encode(path), ret = heap.alloc(8);
  const status = structfs_read(changetype<usize>(p), p.byteLength, ret);
  const out = status == 0 ? takeRet(ret)
            : status == 1 ? null
            : (() => { throw new Error(String.UTF8.decode(takeRet(ret).buffer)); })();
  heap.free(ret);
  return out;
}
// write() is the same shape; everything else is ordinary library code.
```
