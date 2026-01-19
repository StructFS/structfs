# Isotope

A Virtual Operating System built on StructFS semantics.

## What Is This?

Isotope is a specification for a new kind of operating system—one where all
system interaction happens through read and write operations on paths. There
are no system calls, no signals, no sockets. Just stores.

This directory contains the specification, not an implementation. Implementations
like Featherweight (in `../featherweight/`) may realize the specification in
different ways.

## Contents

### Specification (`spec/`)

The normative specification of Isotope concepts:

- [00-overview.md](spec/00-overview.md) — Introduction and principles
- [01-blocks.md](spec/01-blocks.md) — The Block execution primitive
- [02-assemblies.md](spec/02-assemblies.md) — Composition of Blocks
- [03-namespaces.md](spec/03-namespaces.md) — Per-Block path visibility
- [04-system-paths.md](spec/04-system-paths.md) — System services at `/iso/`
- [05-lifecycle.md](spec/05-lifecycle.md) — Block states and transitions
- [06-protocol.md](spec/06-protocol.md) — Store operation semantics
- [07-server-protocol.md](spec/07-server-protocol.md) — How Blocks serve requests
- [08-assembly-management.md](spec/08-assembly-management.md) — Deploying and updating Assemblies

### Rationale (`rationale/`)

Design rationale explaining *why* Isotope is designed this way:

- [01-why-stores.md](rationale/01-why-stores.md) — Why stores all the way down
- [02-why-assemblies.md](rationale/02-why-assemblies.md) — Why Assemblies for composition
- [03-empowering-app-engineers.md](rationale/03-empowering-app-engineers.md) — The goal of Isotope

### Examples (`examples/`)

Example usage patterns:

- [01-hello-world.md](examples/01-hello-world.md) — The simplest Isotope program
- [02-web-service.md](examples/02-web-service.md) — A realistic web service

## Status

This specification is a work in progress. Major open questions are noted in
each document.

## Relationship to Other Projects

- **StructFS** (this repo): Provides the core abstractions (Path, Value, Record,
  Reader, Writer) that Isotope builds on.

- **Featherweight** (`../featherweight/`): A prototype runtime that implements
  (some of) Isotope using WASM and Wasmtime.

## Philosophy

> "Everything is a file" was the revolutionary insight of Unix.
>
> "Everything is a file server" was the insight of Plan 9.
>
> "Everything is a store" is the insight of Isotope.

Isotope exists because we believe application engineers should be able to own
the entire software lifecycle—from ideation through production—without needing
to become infrastructure experts. The path to that goal is making infrastructure
accessible through the same interface as application logic.
