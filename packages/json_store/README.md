# structfs-json-store

JSON-based store implementations for StructFS.

## Stores

### InMemoryStore

In-memory store using the Value type. Data is lost when the store is dropped.

```rust
use structfs_json_store::InMemoryStore;
use structfs_core_store::{Reader, Writer, Record, Value, path};

let mut store = InMemoryStore::new();
store.write(&path!("users/1"), Record::parsed(Value::String("Alice".into())))?;
let record = store.read(&path!("users/1"))?.unwrap();
```

## Value Utilities

The `value_utils` module provides functions for navigating Value trees:

```rust
use structfs_json_store::value_utils::{get_path, set_path};
use structfs_core_store::Value;

let mut tree = Value::Map(/* ... */);
let value = get_path(&tree, &["users", "1", "name"]);
set_path(&mut tree, &["users", "2"], Value::String("Bob".into()))?;
```
