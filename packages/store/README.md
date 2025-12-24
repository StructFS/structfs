# structfs-store

Core traits and types for StructFS stores.

## Key Types

### Traits

- **`Reader`**: Read data from paths via `read_owned<T>()` and `read_to_deserializer()`
- **`Writer`**: Write data to paths via `write<T>()`
- **`Store`**: Combines `Reader + Writer`

### Path

The `Path` type represents locations in the store tree:

```rust
use structfs_store::{Path, path};

let p = Path::parse("users/123")?;
let p = path!("users/123");  // macro (panics on invalid)

// Operations
p.join(&other);
p.has_prefix(&prefix);
p.strip_prefix(&prefix);
```

Paths normalize trailing slashes and double slashes automatically.

### MountStore

Manages multiple stores mounted at different paths:

```rust
use structfs_store::{MountStore, MountConfig};

let mut store = MountStore::new(factory);
store.mount("data", MountConfig::Memory)?;
store.mount("files", MountConfig::Local { path: "/tmp".into() })?;

// Access via /_mounts
store.write(&path!("_mounts/api"), &config)?;
store.read_owned::<Vec<MountInfo>>(&path!("_mounts"))?;
```

### MountConfig

Configuration for different store types:

```rust
pub enum MountConfig {
    Memory,                        // In-memory JSON
    Local { path: String },        // Local filesystem
    Http { url: String },          // HTTP client with base URL
    HttpBroker,                    // HTTP broker for any URL
    Structfs { url: String },      // Remote StructFS server
    Help,                          // Documentation store
}
```

## OverlayStore

Routes operations to stores based on path prefixes:

```rust
use structfs_store::OverlayStore;

let mut overlay = OverlayStore::default();
overlay.add_layer(path!("users"), users_store)?;
overlay.add_layer(path!("config"), config_store)?;

// Reads from /users/123 go to users_store at /123
overlay.read_owned::<User>(&path!("users/123"))?;
```
