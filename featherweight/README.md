# Featherweight

A strawman [Isotope](../isotope/spec/00-overview.md) runtime: blocks are
pico-processes whose entire world is StructFS reads and writes, composed
into assemblies with capability wiring.

## Try it

```console
$ cargo run -p featherweight -- shell
featherweight isotope shell — 'help' for commands
iso> id
"block-70615865-18e5-49f3-9d1d-a656ce141746"
iso> time
"2026-09-03T16:07:41.747291+00:00"
iso> ls iso
log
random
self
server
shutdown
time
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

## What's implemented

- **Blocks** (native Rust or wasm components) with the six lifecycle
  states, exit codes, lazy startup, and graceful→immediate shutdown
  escalation
- **The server protocol and the unified mailbox**: blocks serve their
  stores by reading `iso/server/requests` and writing responses; signals
  and timer deliveries arrive on the same queue (`poll` semantics with
  one primitive); callers park until the response write lands
- **The `/iso/` store** (POSIX closure, spec 09):
  `self/{id,state,args,interface}`, `env`, `stdio/{stdin,stdout,stderr}`,
  `shutdown/{requested,mode,complete}` (with exit codes),
  `time/{now,now_unix_ns,monotonic,zone,after/{ms}}`, `timers`,
  `random/{uuid,int,bytes/{n}}`, `log/{level}`,
  `server/requests[/pending]`, and `proc` (spawn/wait/kill as the handle
  pattern, granted per block via `spawn: true`)
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
- **Wasm blocks**: the WIT boundary is the LL-store boundary (bytes only);
  the `manifest()` export selects the codec before the store bridge exists.
  The Block ABI is single-sourced at `featherweight/wit/world.wit`
- **The WASI tower** (spec 10): the runtime has no WASI dependency —
  WASI is a shim over the Block ABI. `featherweight-wasi` implements the
  syscall core (args/environ/clocks/random/stdio/exit/errno) generically
  over any store; `tests/wasi_tower.rs` runs a POSIX-style program end
  to end on the `/iso/` surface

## Strawman limits

Documented in `docs/plans/2026-09-03-handles-and-featherweight.md`:
JSON serialization only, no hash verification or registries, no
`extends`, no restart policy, no deadlock detection, and the shell talks
to the terminal directly (the store model has no tty story yet).
