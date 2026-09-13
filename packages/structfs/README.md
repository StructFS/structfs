# StructFS

A uniform interface for accessing data through read/write operations on paths.

StructFS treats everything as a store - local files, remote APIs, in-memory
data, and even the mount configuration itself are all accessed through the same
read/write interface.

## Library quickstart

Published baseline: **0.2.0**. This checkout targets **0.3.0 (unreleased)**;
see the [0.3 migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.3.md)
and [platform matrix](https://github.com/StructFS/structfs/blob/main/docs/platforms.md).

The published quickstart uses:

```toml
[dependencies]
structfs = { version = "0.2.0", features = ["json"] }
structfs-core-store = "0.2.0" # required by the path! expansion
```

```rust
use structfs::{path, InMemoryStore, Reader, Record, Value, Writer};

fn main() -> Result<(), structfs::Error> {
    let mut store = InMemoryStore::new();
    store.write(&path!("greeting"), Record::parsed(Value::String("hello".into())))?;
    assert!(store.read(&path!("greeting"))?.is_some());
    Ok(())
}
```

Default features expose only the core contract. Opt into `serde`, `json`, `http`,
`sys`, `async`, `service`, `state`, or `profiles`; `full` enables them all.
Rust 1.96+ is supported. Higher-level consistency, persistence, observation and
process guarantees belong to each store's documented contract.

See the [changelog](https://github.com/StructFS/structfs/blob/main/CHANGELOG.md),
[migration guide](https://github.com/StructFS/structfs/blob/main/docs/migration-0.2.md),
and [release procedure](https://github.com/StructFS/structfs/blob/main/docs/releasing.md).
