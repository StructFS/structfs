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

## What's implemented

- **Blocks** (native Rust or wasm components) with the six lifecycle
  states, lazy startup, and graceful→immediate shutdown escalation
- **The server protocol**: blocks serve their stores by reading
  `iso/server/requests` and writing responses; callers park until the
  response write lands
- **The `/iso/` store**: `self/{id,state,interface}`,
  `shutdown/{requested,mode,complete}`, `time/{now,monotonic,zone}`,
  `random/{uuid,int,bytes/{n}}`, `log/{level}`, `server/requests[/pending]`
- **Assemblies**: JSON/YAML definitions, component-wise wiring with
  bidirectional path rewriting, read-only `/config` injection, imports,
  fail-fast/isolate failure policies, and nested assembly definitions
  (an assembly is a block — the fractal property)
- **Wasm blocks**: the WIT boundary is the LL-store boundary (bytes only);
  the `manifest()` export selects the codec before the store bridge exists

## Strawman limits

Documented in `docs/plans/2026-09-03-handles-and-featherweight.md`:
JSON serialization only, no hash verification or registries, no
`extends`, no restart policy, no deadlock detection, and the shell talks
to the terminal directly (the store model has no tty story yet).
