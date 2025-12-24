# structfs-json-store

JSON-based store implementations for StructFS.

## Stores

### SerdeJSONInMemoryStore

In-memory JSON store. Data is lost when the store is dropped.

```rust
use structfs_json_store::in_memory::SerdeJSONInMemoryStore;
use structfs_store::{Reader, Writer, path};

let mut store = SerdeJSONInMemoryStore::new()?;
store.write(&path!("users/1"), &user)?;
let user: User = store.read_owned(&path!("users/1"))?.unwrap();
```

### JSONLocalStore

Persists JSON to the local filesystem. Each path component becomes a directory, with data stored in `_record.json` files.

```rust
use structfs_json_store::JSONLocalStore;
use std::path::PathBuf;

let mut store = JSONLocalStore::new(PathBuf::from("/data/mystore"))?;
store.write(&path!("config/app"), &config)?;
// Creates: /data/mystore/config/app/_record.json
```

## JSON Utilities

The `json_utils` module provides functions for navigating JSON trees:

```rust
use structfs_json_store::json_utils::{get_path, set_path};

let mut tree = json!({"users": {"1": {"name": "Alice"}}});
let value = get_path(&tree, &path!("users/1/name"))?;
set_path(&mut tree, &path!("users/2"), json!({"name": "Bob"}))?;
```
