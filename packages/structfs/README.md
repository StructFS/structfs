# StructFS

A uniform interface for accessing data through read/write operations on paths.

StructFS treats everything as a store - local files, remote APIs, in-memory
data, and even the mount configuration itself are all accessed through the same
read/write interface.

## Library quickstart

This README describes the **0.5 development line**; 0.4.0 is the published
release. See the [changelog](https://github.com/StructFS/structfs/blob/main/CHANGELOG.md)
and [platform matrix](https://github.com/StructFS/structfs/blob/main/docs/platforms.md).

```toml
[dependencies]
structfs = { version = "0.5.0", features = ["persist"] }
```

The `path!` macro expands hygienically through the facade, so no direct
`structfs-core-store` dependency is needed.

```rust
use structfs::{path, MemoryStore, Reader, Record, Value, Writer};

fn main() -> Result<(), structfs::Error> {
    let mut store = MemoryStore::new();
    store.write(&path!("greeting"), Record::parsed(Value::from("hello")))?;
    assert!(store.read(&path!("greeting"))?.is_some());
    Ok(())
}
```

Typed access and durable storage live in their feature modules:

```rust,no_run
use structfs::persist::{BackedStore, JsonFileBacking};
use structfs::typed::{TypedReader, TypedWriter};
use structfs::path;

fn main() -> Result<(), structfs::Error> {
    let mut store = BackedStore::open(JsonFileBacking::new("config.json"))?;
    store.write_typed(&path!("server/port"), &8080u16)?;
    let port: Option<u16> = store.read_typed(&path!("server/port"))?;
    assert_eq!(port, Some(8080));
    Ok(())
}
```

## Layout

Every item has one path. The root holds the core vocabulary — `Reader`,
`Writer`, `Store`, `Path`, `PathComponent`, `path!`, `Value`, `Record`,
`Format`, `Error`, `MemoryStore`, the combinators, `MountStore`,
`OverlayStore` and `conformance` — plus the async trait families when `async`
is enabled. `Bytes` is the `bytes` crate's type, re-exported because
`Record::raw` takes it. The byte layer is `structfs::ll`, path matching is
`structfs::pattern`. Every feature-gated area is a module named after its
feature, and its items appear only there.

Module names never shadow well-known crates, so `use structfs::*;` sits beside
`use serde::Serialize;` without ambiguity.

## Features

Default features expose only the core contract.

| Feature | Module | Adds | Implies |
| --- | --- | --- | --- |
| `async` | root | Async and detached trait families; async helpers in `typed` | |
| `typed` | `typed` | Serde-typed access, codecs (`JsonCodec`, `CborCodec`, `FlexbuffersCodec`, `ValueCodec`, `MultiCodec`, `CodecProfile`), `Limits`, `transcode` | |
| `persist` | `persist` | `BackedStore`, `LogStore` and their backings (`JsonFileBacking`, `JsonlFileBacking`, `MemoryAppendBacking`, `Durability`) | `typed` |
| `net` | `net` | Portable HTTP schema: `HttpRequest`, `HttpResponse`, `Method`, request status, `sse`, errors | |
| `net-blocking` | `net` | Native HTTP stores and `BlockingReqwestExecutor` | `net` |
| `net-streaming` | `net::streaming` | Native async streaming executor | `net`, `async` |
| `os` | `os` | `SysStore` — env, time, random, proc, fs, docs | |
| `handles` | `handles` | Handle stores, gates, cancellation, duplex streams | `async` |
| `service` | `service` | Shared async routing and owned providers | `handles`, `async` |
| `state` | `state` | Revisioned state and bounded observation | `service` |
| `profiles` | `profiles` | Capability profiles and headless interactive sessions | `service` |
| `full` | all | Every feature above | |

The core, `async`, `typed`, `net` and `handles` build for
`wasm32-unknown-unknown`; `net-blocking` and `net-streaming` pull in a native
HTTP client. Rust 1.96+ is supported. Higher-level consistency, persistence,
observation and process guarantees belong to each store's documented contract.

See the [changelog](https://github.com/StructFS/structfs/blob/main/CHANGELOG.md),
[0.5 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.5.md)
(unreleased line; the [0.4 guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.4.md)
covers the published release),
and [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md).
