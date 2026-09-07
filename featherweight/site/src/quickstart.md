---
layout: base.njk
title: Quickstart
permalink: /quickstart/
templateClass: doc-page
---

<div class="doc-page">

# Quickstart

Featherweight lives in the
[StructFS repository](https://github.com/StructFS/structfs). Everything
below runs from a clone:

```bash
git clone https://github.com/StructFS/structfs
cd structfs
```

## The demo shell

```bash
cargo run -p featherweight -- shell
```

`fw shell` instantiates the demo assembly: an interactive shell block
wired to kv, echo, and logger service blocks. Inside it, every command
is a store operation on the shell's namespace:

```text
iso> write /services/kv/greeting "hello"
-> /services/kv/greeting
iso> read /services/kv/greeting
"hello"
iso> read /iso/time/now
```

The shell has no special powers — `/services/kv` works because the
assembly wired it, and only that. Unwired paths are denied.

## Assemblies

`fw run` takes an assembly definition (JSON or YAML). The demo assembly,
spelled out ([featherweight/demo.assembly.yaml](https://github.com/StructFS/structfs/blob/main/featherweight/demo.assembly.yaml)):

```yaml
assembly: fw-demo
version: "0.1.0"

blocks:
  shell:
    artifact: builtin:shell
    stdio: host
    spawn: true
    env:
      DEMO: "1"
    args: ["shell"]
  kv: builtin:kv
  echo: builtin:echo
  logs: builtin:logger

public: shell

wiring:
  - "shell:/services/kv -> kv"
  - "shell:/services/echo -> echo"
  - "shell:/services/logs -> logs"

config:
  shell:
    prompt: "iso> "

failure:
  kv: isolate
```

The `wiring` lines are the capability grants: block-local path prefixes
mapped to sibling blocks (or `$imports` from the parent). An assembly is
itself a block, so definitions nest — the fractal property
([spec 02](https://isotope.structfs.com/spec/assemblies/)).

## Your first wasm block

A block compiles to a plain wasm core module — no componentization, no
bindgen. The reference kv block:

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release -p featherweight-guest
```

Drop the artifact into an assembly:

```yaml
assembly: kv-demo
blocks:
  shell:
    artifact: builtin:shell
    stdio: host
    args: ["shell"]
  kv: kv.wasm
public: shell
wiring:
  - "shell:/services/kv -> kv"
```

```bash
cp target/wasm32-unknown-unknown/release/featherweight_guest.wasm kv.wasm
cargo run -p featherweight -- run kv-demo.yaml
```

The wasm block serves reads and writes over the [server
protocol](https://isotope.structfs.com/spec/server-protocol/) — its
whole interface to the world is the two-function
[Block ABI](https://isotope.structfs.com/abi/). The same artifact runs
unmodified under the [browser host](/demo/).

## What a block sees

Inside a block, `/iso/` is the syscall surface
([spec 04](https://isotope.structfs.com/spec/system-paths/)):

| Path | Meaning |
|------|---------|
| `iso/self/args`, `iso/env` | identity |
| `iso/time/now_unix_ns`, `iso/time/monotonic` | clocks |
| `iso/random/bytes/{n}` | randomness |
| `iso/stdio/stdin`, `iso/stdio/stdout` | standard streams |
| `iso/server/requests` | the mailbox — reading it *is* serving |
| `iso/shutdown/complete` | declare your exit |

POSIX-style programs get all of this through the WASI shim
(`featherweight-wasi`), which is a guest-side compatibility layer over
the same paths — the runtime itself has no WASI dependency
([spec 10](https://isotope.structfs.com/spec/wasi-tower/)).

</div>
