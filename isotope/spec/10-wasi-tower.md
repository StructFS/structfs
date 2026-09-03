# The WASI Tower

Isotope does not depend on WASI. The Block ABI (below) is the only
interface a runtime provides; every WASI version is a **compatibility
layer** — a shim bound to the guest at load time — that bottoms out in
that ABI. WASI is a protocol encoded in StructFS, not a platform beside
it.

## The Block ABI

The canonical interface is the WIT world in `featherweight/wit/world.wit`
(single-sourced; host and guest bindings are both generated from it):

```wit
// guest imports (the entire system interface)
read:  func(path: list<list<u8>>) -> result<option<list<u8>>, string>
write: func(path: list<list<u8>>, data: list<u8>) -> result<list<list<u8>>, string>

// guest exports
manifest: func() -> list<u8>     // JSON self-description, pre-wiring
run:      func() -> result<_, string>
```

Properties the ABI guarantees:

- **Byte-level, format-agnostic**: paths are validated byte components;
  data is bytes in the Block's manifest-declared serialization. The ABI
  never changes when formats do.
- **Stateless calls**: each call is complete in itself — no pending
  results, no call-ordering protocol, no host-side per-call state.
- **Capability-complete**: everything reachable is reachable through the
  namespace. There is no second interface to audit.

Planned ABI v2 (not yet adopted): errors as a WIT `variant` mirroring
the protocol error taxonomy (spec 06), so typed errors survive the
boundary; today they collapse to strings at the wasm edge only —
native shims see the typed errors directly.

## The Tower

One canonical shim, adapters stacked above it:

```
preview1 binary ──(official p1→p2 adapter, wasm-tools --adapt)──▶ p2
p2 binary ──────────────────────────────────────────────────────▶ p2
                                                                  │
                                        wasi-over-isotope shim (guest side)
                                                                  │
                                                       read / write (Block ABI)
                                                                  │
                                              /iso/* + wired mounts
future 0.3 (async/streams) ──▶ a thin layer over the mailbox + handle stores
```

The shim lives **in the guest** (composed/swizzled at load time), never
in the host:

- The runtime's trusted surface stays two functions; no WASI code or
  state in the TCB. The fd table lives in guest memory — a corrupt fd
  table harms only its own Block.
- The shim is a versioned, content-addressed artifact like any Block;
  different Blocks may carry different WASI versions, and upgrading the
  shim never touches the runtime.
- The shim compiles against `read`/`write` only, so any host offering
  the Block ABI — other runtimes, browsers — runs WASI programs with a
  small binding layer.

Loaders MAY perform the adapt/compose step at instantiation, driven by
a Block-definition field (`wasi: {version, preopens}`), so users ship
stock binaries.

## The Syscall Mapping

| WASI | Isotope surface |
|---|---|
| `args_get`, `environ_get` | `/iso/self/args`, `/iso/env` |
| `clock_time_get` (realtime / monotonic) | `/iso/time/now_unix_ns`, `/iso/time/monotonic` |
| `random_get` | `/iso/random/bytes/{n}` |
| fd 0 / 1 / 2 | `/iso/stdio/{stdin,stdout,stderr}` |
| `proc_exit(code)` | `write /iso/shutdown/complete {"code": N}` |
| `poll_oneoff` | the mailbox; pure clock subscriptions via `/iso/time/after/{ms}` |
| preopens, `path_open`, `fd_*` | **wired** filesystem mounts + the byte-stream pattern (`docs/patterns/bytestream.md`); a preopen IS a namespace mount |
| `sock_*` | wired network stores, the handle pattern |
| rights/fdstat | derived from the meta lens (`readable`/`writable`/`blocking`) |
| unmappable (symlink semantics the store lacks, etc.) | honest `ENOTSUP` |

### errno

The protocol error taxonomy maps per spec 09: `not_found`→`ENOENT`,
`forbidden`→`EACCES`/`ENOTCAPABLE`, `unavailable`→`EAGAIN`,
`timeout`→`ETIMEDOUT`, `conflict`→`EEXIST`, cancellation→`EINTR`,
`invalid_path`→`EINVAL`, unmapped store errors→`EIO`.

### Blocking

Preview1's synchronous calls map directly onto parked reads on the
Block's thread; which paths may park is declared at `/iso/meta`.
Preview 0.3's async model maps onto the mailbox. Both ends of WASI's
own evolution land on existing surfaces.

## Capability Semantics

WASI's preopen/rights system is not implemented as a second capability
model; it is a **projection** of the namespace. A preopened directory is
a wired mount; a rights mask is read off the meta lens; a path the
Assembly didn't wire is `ENOTCAPABLE` — which is what WASI's own
capability story always wanted to mean.

## Reference Shim

`featherweight-wasi` implements the syscall core **natively and
generically** over any StructFS store (the fd table, errno mapping, and
the surface above), so the mapping is unit-testable against a real
Block namespace without a wasm toolchain. The wasm packaging — the same
core behind `wasi_snapshot_preview1` exports, composed onto stock
binaries — is the load-time layer, merged from the shim lineage
(appiware's `wasi_shim`/`preview1`, whose fd table ran CPython over
StructFS).

## Costs, Accepted

- Every syscall is a boundary crossing plus a store operation; chatty
  programs pay for it. Buffered fds in the shim and ranged byte reads
  bound the damage; a cached-path fast path is the known future
  optimization.
- Conformance long-tail (fd renumbering, dirent cookies) is quarantined
  in the shim and scored by the WASI test suites.
- The compose step exists, but as a loader concern, not a user-facing
  toolchain.
