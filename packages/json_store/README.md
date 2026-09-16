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

## File acknowledgement and failed writes

`JsonFileBacking` and `JsonlFileBacking` default to `Durability::Buffered`: an
acknowledgement means OS writes completed, not power-loss durability. Select
`with_durability(Durability::Synced)` to synchronize file contents and the parent
directory before acknowledgement. Synced mode requires an existing parent and
filesystem support for these operations. Use one serialized writer.

`BackedStore` and `LogStore` publish memory only after backing acknowledgement.
A backing error leaves the previous memory state readable and fences further
writes. `recover()` explicitly reloads disk; it may adopt an unacknowledged write.
Do not blindly retry non-idempotent appends. JSONL requires newline-terminated
records and rejects incomplete tails rather than silently truncating them.
