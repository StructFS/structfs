---
layout: base.njk
title: Getting Started
templateClass: doc-page
---

# Getting Started

## Install

Install the REPL from the [reference implementation](/implementations/) to follow along, or use the [playground](/playground/) in your browser.

## Your first store

Launch the REPL and write some data:

```bash
$ structfs
> write /data/greeting "Hello, World!"
Written to: /data/greeting

> read /data/greeting
"Hello, World!"
```

A memory store is already mounted at `/data`. You wrote a string to a path, then read it back. That's the entire interface — `read` and `write`.

## Structured data

Values aren't just strings. Write a map:

```bash
> write /data/users/alice {"name": "Alice", "email": "alice@example.com"}
Written to: /data/users/alice

> read /data/users/alice
{"name": "Alice", "email": "alice@example.com"}
```

StructFS deals in structured values — strings, numbers, booleans, arrays, maps. No file formats, no parsing.

## Everything is a path

The environment, the clock, a random number generator — they're all stores, accessed through paths:

```bash
> read /ctx/sys/env/HOME
"/Users/alice"

> read /ctx/sys/time/now
"2025-01-15T10:30:00Z"

> read /ctx/sys/random/uuid
"550e8400-e29b-41d4-a716-446655440000"
```

This isn't syntactic sugar. These are real stores mounted at `/ctx/sys`, responding to `read` the same way your data store does.

## Mount more stores

Want another store? Mount one:

```bash
> write /ctx/mounts/scratch {"type": "memory"}
Written to: /ctx/mounts/scratch

> write /scratch/temp "this is ephemeral"
> read /scratch/temp
"this is ephemeral"
```

The mount system itself is accessed through `read` and `write` at `/ctx/mounts`. Even configuration uses the same interface.

## Make an HTTP request

The HTTP broker turns network requests into the same read/write pattern:

```bash
> write /ctx/http {"method": "GET", "path": "https://httpbin.org/get"}
Written to: /ctx/http/outstanding/0

> read /ctx/http/outstanding/0
{"status": 200, "headers": {...}, "body": {...}}
```

Write queues the request. Read executes it. The interface stays synchronous.

## Next steps

- [Concepts](/concepts/) — the model behind the interface
- [Stores](/stores/) — store design in depth
- [Patterns](/patterns/) — references, pagination, and introspection
