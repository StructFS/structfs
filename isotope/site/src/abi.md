---
layout: base.njk
title: The Block ABI
permalink: /abi/
templateClass: doc-page
---

<div class="doc-page">

# The Block ABI

The entire interface between a block and its runtime is two functions:

```wat
(import "structfs" "read"
  (func (param $path_ptr i32) (param $path_len i32)
        (param $ret_ptr i32) (result i32)))          ;; status

(import "structfs" "write"
  (func (param $path_ptr i32) (param $path_len i32)
        (param $data_ptr i32) (param $data_len i32)
        (param $ret_ptr i32) (result i32)))          ;; status
```

There is no third function. Process control, IPC, stdio, clocks,
randomness, serving requests, shutdown — all of it is reads and writes
on paths in the block's namespace. The ABI is a **semantic contract**
([spec 10](/spec/wasi-tower/)), projected into bindings; no binding is
the definition.

## The bindings

| Binding | What | When |
|---------|------|------|
| **Native** | The `NativeBlock` trait over a namespace store | Rust blocks in-process; errors flow fully typed |
| **Core wasm** ([spec 11](/spec/core-wasm-binding/)) | The two imports above over linear memory | The SDK binding: any language that targets wasm, no tooling |
| **Component model** | A WIT world derived from the contract | An *adapter* (`featherweight-component`) for wasip2/wit-bindgen artifacts — the runtime core has zero idea what WIT is |
| **Wire** | Network transports carrying the same operations | Planned |

## Status codes

The typed error taxonomy crosses the boundary as the return status:

| Status | Meaning |
|-------:|---------|
| `0` | ok — payload in the ret record |
| `1` | absent (a read found nothing) |
| `-1` | not found |
| `-2` | permission denied (an unwired path) |
| `-3` | conflict |
| `-4` | overloaded |
| `-5` | deadline exceeded |
| `-6` | cancelled |
| `-7` | invalid path |
| `-8` | resource limit |
| `-9` | other |

Payloads travel in the block's manifest-declared serialization — JSON,
CBOR, and FlexBuffers are supported as equals.

## An SDK is an afternoon

The guest side needs exactly one export beyond its entry points: a
`block_alloc(len) -> ptr` the host uses to deliver results. This is a
complete, working guest, hand-written in the least ergonomic language
available:

```wat
(module
  (import "structfs" "read"
    (func $read (param i32 i32 i32) (result i32)))
  (import "structfs" "write"
    (func $write (param i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 4096))
  (func (export "block_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (data (i32.const 1040) "input")
  (data (i32.const 1072) "output")
  (data (i32.const 1088)
    "{\"name\":\"wat-echo\",\"serialization\":\"application/json\"}")
  (func (export "manifest") (param $ret i32) (result i32)
    (i32.store (local.get $ret) (i32.const 1088))
    (i32.store (i32.add (local.get $ret) (i32.const 4)) (i32.const 54))
    (i32.const 0))
  (func (export "run") (result i32)
    ;; read "input", echo the bytes to "output"
    (drop (call $read (i32.const 1040) (i32.const 5) (i32.const 1024)))
    (drop (call $write (i32.const 1072) (i32.const 6)
      (i32.load (i32.const 1024)) (i32.load (i32.const 1028))
      (i32.const 1032)))
    (i32.const 0)))
```

In a language with an allocator it is smaller. The shape of the
AssemblyScript SDK ([spec 11](/spec/core-wasm-binding/) carries the
full sketch):

```ts
@external("structfs", "read")
declare function structfs_read(p: usize, pl: i32, ret: usize): i32;
@external("structfs", "write")
declare function structfs_write(p: usize, pl: i32, d: usize, dl: i32, ret: usize): i32;

export function block_alloc(len: i32): usize { return heap.alloc(len); }

export function read(path: string): Uint8Array | null {
  const p = String.UTF8.encode(path), ret = heap.alloc(8);
  const status = structfs_read(changetype<usize>(p), p.byteLength, ret);
  // 0 -> bytes, 1 -> null, negative -> throw the host's message
  ...
}
```

## Hosts are small too

The reference hosts of the core binding:

- **featherweight** (wasmtime) — the native runtime
- **the browser host** — ~120 lines of dependency-free JavaScript. The
  same `kv.wasm` runs resident in a Web Worker, its mailbox read parked
  in `Atomics.wait`. [It's running on the featherweight
  site.](https://featherweight.structfs.com/demo/)

That the same guest binary runs under both is the host-neutrality the
binding claims — demonstrated, not asserted.

</div>
