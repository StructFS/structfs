# Featherweight

A strawman [Isotope](../isotope/spec/00-overview.md) runtime: blocks are
pico-processes whose entire world is StructFS reads and writes, composed
into assemblies with capability wiring.

## Install the 0.4 release line

0.4.0 is published; this checkout is the 0.5 development line (see the
[0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)).
The [registry status record](https://github.com/StructFS/structfs/blob/main/docs/release-status.md)
records exact-version availability; the
[changelog](https://github.com/StructFS/structfs/blob/main/CHANGELOG.md) lists what changed.

```sh
cargo install featherweight --version 0.4.0 --locked
fw shell
```

Rust 1.96+ is supported. Embedders depend on `featherweight-runtime`; core-Wasm
guests depend on `featherweight-guest`. The component-model adapter and WASI shim
are separate crates. See the
[migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md)
and [specification matrix](https://github.com/StructFS/structfs/blob/main/isotope/RELEASE.md)
for the release line’s compatibility and support boundaries.

## Try it

```console
$ cargo run -p featherweight -- shell
featherweight isotope shell — 'help' for commands
iso> id
"block:fw-demo/shell"
iso> time
"2026-09-03T16:07:41.747291+00:00"
iso> ls iso
capabilities
env
execution
log
meta
proc
random
self
server
shutdown
stdio
time
timers
iso> write services/kv/greeting {"text": "hello isotope"}
-> services/kv/greeting
iso> read services/kv/greeting
{
  "text": "hello isotope"
}
iso> log warn shutting down now
[warn] shell: shutting down now
iso> exit
```

Every shell command is a store operation on the shell block's namespace.
`services/kv` is a separate block reached through the server protocol;
`iso/` is the runtime's syscall surface.

Run an assembly definition:

```console
$ cargo run -p featherweight -- run featherweight/demo.assembly.yaml
```

Try `spawn {"assembly": "child", "blocks": {"kv": "builtin:kv"}, "public": "kv"}`
in the shell: spawn(2) is a write to `iso/proc`, wait(2) is a blocking
read of the returned handle, and kill(2) is a Null write to it.

`fw --help` lists the orthogonal run flags, accepted before or after the
subcommand: `--record DIR`, `--replay DIR`, `--seek DIR [--at SEQ]`
(transcripts), `--seed N` or `--sim N` (determinism), and `--session FILE`
(the forensic session log).

## What's implemented

- **Blocks** (native Rust or core-wasm; WIT components through the
  `featherweight-component` adapter) with the six lifecycle
  states, exit codes, lazy startup, and graceful→immediate shutdown
  escalation
- **The server protocol and the unified mailbox**: blocks serve their
  stores by reading `iso/server/requests` and writing responses; signals
  and timer deliveries arrive on the same queue (`poll` semantics with
  one primitive); callers park until the response write lands
- **The `/iso/` store** (POSIX closure, spec 09; embedding, spec 13):
  `self/{id,state,args,interface,last_error}`, `env`,
  `stdio/{stdin,stdout,stderr}`, `shutdown/{requested,mode,complete}`
  (with exit codes), `time/{now,now_unix_ns,monotonic,zone,after/{ms}}`,
  `timers`, `random/{uuid,int,bytes/{n}}`, `log/{level}`,
  `server/requests[/pending]`, `server/responses/{token}`,
  `server/cancelled/{token}`, `capabilities`, `execution/budget`, `meta`
  (which describes every path above), and `proc` (spawn/wait/kill as the
  handle pattern, granted per block via `spawn: true`)
- **Assemblies**: JSON/YAML definitions, component-wise wiring with
  bidirectional path rewriting, read-only `/config` injection, per-block
  `env`/`args`/`stdio`, imports, fail-fast/isolate failure policies, and
  nested assembly definitions (an assembly is a block — the fractal
  property)
- **Management as a store**: `Runtime::management_store()` deploys,
  observes, and shuts down assemblies through the same spawn protocol —
  no separate management API
- **Capability discipline**: unwired paths are denied (reads and writes
  alike); filesystem/network access is granted by wiring, never ambient
- **Wasm blocks**: the core runtime speaks exactly one wasm binding —
  the **core-wasm binding** (spec 11): two imports in the `structfs`
  module, no bindgen or component tooling, plain `cargo build --target
  wasm32-unknown-unknown` output runs directly
  (`./scripts/run_wasm_block.sh` for the live demo; hand-written wat
  guests in the tests). Other bindings are **adapters** registered via
  `RuntimeConfig::register_loader`: `featherweight-component` teaches the
  runtime to run WIT component-model artifacts as blocks (the `fw` CLI
  registers it; the core has zero idea what WIT is).
  `featherweight/guest` is the Rust core-binding SDK, with the reference
  kv block behind its `reference-guest` feature;
  `featherweight/sdk/assemblyscript` is the ~60-line AssemblyScript SDK.
  The `manifest()` export selects the codec before the store bridge exists
  in every binding
- **Metering**: a per-run fuel cap (`Metering`) honoured by both wasm
  bindings, and engine-wide epoch interruption (10 ms by default) that
  lets immediate shutdown and deadlines stop a spinning guest
- **The browser host** (`featherweight/host/browser`): the core binding
  hosted in TypeScript with no runtime dependencies — the same `kv.wasm`
  runs resident in a Web Worker, its mailbox read parked in `Atomics.wait`
  (`./scripts/browser_host_test.sh` for the Node tests; `npm run build &&
  node serve.ts` in `featherweight/host/browser` for the live demo)
- **The WASI tower** (spec 10): the runtime has no WASI dependency —
  WASI is a shim over the Block ABI. `featherweight-wasi` implements the
  syscall core (args/environ/clocks/random/stdio/exit/errno) generically
  over any store; `tests/wasi_tower.rs` runs a POSIX-style program end
  to end on the `/iso/` surface

## Strawman limits

Documented in the (historical) plan
`docs/history/plans/2026-09-03-handles-and-featherweight.md`:
no hash verification or registries, no `extends`, no restart policy, and
deadlock detection only under `Determinism::Simulation` (`fw --sim N`: the
turnstile scheduler reports call-dependency cycles; the default
OS-scheduled `Live` and `Seeded` modes have none). The shell has no out-of-band terminal channel — all of its
I/O goes through `iso/stdio`, which the `fw` CLI wires to the process
terminal; the store model has no tty story beyond line-oriented stdio.
Blocks declare their transport in
the manifest — JSON, CBOR, and FlexBuffers are supported equivalently
(`MultiCodec::standard()`); cross-format translation between blocks is
untested, and the browser host speaks JSON only.
