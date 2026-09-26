# structfs-json-store

Durable StructFS stores: a whole-file snapshot and an append-only log.

The in-memory semantics come from `structfs_core_store::MemoryStore`. This
crate adds *where the bytes live* and *when a write is acknowledged* — not new
store conventions.

| Type | Shape | Use |
|------|-------|-----|
| `BackedStore<B: Backing>` | Tree, rewritten whole on every write | Config, small state |
| `LogStore<B: AppendBacking>` | Append-only sequence | Ledgers, journals, transcripts |

Each is generic over its persistence trait, so the medium is swappable:

```rust
pub trait Backing: Send + Sync {
    fn load(&mut self) -> Result<Option<Value>, Error>;
    fn save(&mut self, root: &Value) -> Result<(), Error>;
}

pub trait AppendBacking: Send + Sync {
    fn load(&mut self) -> Result<Vec<Value>, Error>;
    fn append(&mut self, entry: &Value) -> Result<(), Error>;
}
```

Implementations shipped here: `JsonFileBacking` (a snapshot file),
`JsonlFileBacking` (a log file) and `MemoryAppendBacking` (an ephemeral log,
and the test double).

## Snapshots

```rust
use structfs_json_store::{BackedStore, Durability, JsonFileBacking};
use structfs_core_store::{path, Reader, Record, Value, Writer};

let mut store = BackedStore::open(
    JsonFileBacking::new("config.json").with_durability(Durability::Synced),
)?;
store.write(&path!("server/port"), Record::parsed(Value::from(8080i64)))?;
let port = store.read(&path!("server/port"))?.unwrap();
```

`BackedStore::root() -> Option<&Value>` borrows the in-memory tree; `None` is
an empty store.

## Append-only logs

`LogStore` serves a log as a store. Everything but `append` is read-only:

| Path | Operation | Result |
|------|-----------|--------|
| `write append <value>` | Append an entry | Returns `entries/{n}` |
| `write append null` | — | `InvalidArgument` (see below) |
| `write <anything else>` | — | `PermissionDenied` |
| `read ` (root) | All entries | `Value::Array` |
| `read len` | Entry count | `Value::Integer` |
| `read entries/{n}` | One entry | The entry, or `None` past the end |
| `read entries/{x}` | Non-numeric `{x}` | `InvalidArgument` |
| `read entries/from/{n}` | Tail from a cursor | `{items, next, status}` |
| `read entries/from/{x}` | Non-numeric `{x}` | `InvalidArgument` |

```rust
use structfs_json_store::{JsonlFileBacking, LogStore};
use structfs_core_store::{path, Reader, Record, Value, Writer};

let mut log = LogStore::open(JsonlFileBacking::new("ledger.jsonl"))?;
let at = log.write(&path!("append"), Record::parsed(Value::from("started")))?;
// at == entries/0
let page = log.read(&path!("entries/from/0"))?.unwrap();
// {"items": [...], "next": 1, "status": "open"}
```

The tail read returns an `{items, next, status}` envelope, so consumers page
with a cursor instead of re-reading the whole ledger. A log has no terminal state, so `status` is always
`"open"`.

**Writing `Null` to `append` is rejected.** It is the one place in this crate
where `Null` does not mean deletion: an append-only log has nothing to delete,
and a stored `Null` entry would read back indistinguishably from an absent one.

## File acknowledgement and failed writes

`JsonFileBacking` and `JsonlFileBacking` default to `Durability::Buffered`: an
acknowledgement means OS writes completed, not power-loss durability. Select
`with_durability(Durability::Synced)` to synchronize file contents and the parent
directory before acknowledgement. Synced mode requires an existing parent and
filesystem support for these operations. Use one serialized writer.

`BackedStore` and `LogStore` publish memory only after backing acknowledgement.
A backing error leaves the previous memory state readable. Only a failure that
may already have touched disk (`last_failure_ambiguous()`) fences further
writes; failures before that point (limits, encoding, temp file, a rejected
partial tail) leave the store writable. `needs_recovery()` reports the fence. `recover()` explicitly reloads
disk; it may adopt an unacknowledged write. Do not blindly retry non-idempotent
appends. The log file requires newline-terminated records and rejects incomplete
tails rather than silently truncating them.

## On-disk format

Both file backings write **StructFS Value JSON v1** — the
`["structfs-value",1,…]` envelope from `structfs_serde_store::ValueJsonCodec` —
a whole document for a snapshot, one per line for a log.

Before 0.5 they wrote plain `serde_json`, which silently turns `Value::Bytes`
into an array of numbers and NaN/±inf into `null`. Reads accept both: a
document is tried as the tagged form first and then as plain JSON, so files
written by 0.4 and earlier still load. The order matters in one corner: a legacy
document that is literally the array `["structfs-value", 1, <valid tagged body>]`
reads as the tagged form. Writes are always tagged, so **a file this version has
written cannot be read back by 0.4**.

Loads **and saves** are bounded by the backing's `Limits` —
`Limits::default()` (16 MiB, depth 64, 65536 entries per collection) unless set
with `JsonFileBacking::with_limits` / `JsonlFileBacking::with_limits`. A save or
append the bounds reject fails before the file is touched, so the store stays
writable and unchanged (`Backing::last_failure_ambiguous` /
`AppendBacking::last_failure_ambiguous` tell the store whether a failure needs
`recover()`). The tagged form carries at most 84 levels of nesting whatever the
limits; a deeper legacy file loads, but cannot be saved again.
