//! # structfs-json-store
//!
//! Durable StructFS stores: a whole-file snapshot and an append-only log.
//!
//! The in-memory semantics come from [`structfs_core_store::MemoryStore`] —
//! this crate adds *where the bytes live* and *when a write is
//! acknowledged*, not new store conventions.
//!
//! | Type | Shape | Use |
//! |------|-------|-----|
//! | [`BackedStore`] | Tree, rewritten whole on every write | Config, small state |
//! | [`LogStore`] | Append-only sequence | Ledgers, journals, transcripts |
//!
//! Each is generic over its persistence trait, so the storage medium is
//! swappable: [`Backing`] for snapshots ([`JsonFileBacking`] is the file
//! implementation) and [`AppendBacking`] for logs ([`JsonlFileBacking`] for
//! files, [`MemoryAppendBacking`] for an ephemeral log or a test double).
//!
//! ```rust,no_run
//! use structfs_json_store::{BackedStore, JsonFileBacking, Durability};
//! use structfs_core_store::{path, Reader, Record, Value, Writer};
//!
//! let mut store = BackedStore::open(
//!     JsonFileBacking::new("config.json").with_durability(Durability::Synced),
//! )?;
//! store.write(&path!("server/port"), Record::parsed(Value::from(8080i64)))?;
//! assert_eq!(
//!     store.read(&path!("server/port"))?.unwrap().as_value(),
//!     Some(&Value::from(8080i64)),
//! );
//! # Ok::<(), structfs_core_store::Error>(())
//! ```
//!
//! # Durability and recovery
//!
//! [`Durability::Buffered`] (the default) acknowledges once the operating
//! system has the bytes; [`Durability::Synced`] synchronizes the file and its
//! parent directory first. Both file backings write so that a crash leaves
//! either the old state or the new one, never a half-written tree: snapshots
//! go to a temp file that is renamed over the target, and log entries are
//! whole newline-terminated lines, with a partial trailing line rejected
//! rather than silently truncated.
//!
//! A save or append that fails *after* touching the disk is ambiguous. Both
//! stores then fence further writes and report [`BackedStore::needs_recovery`]
//! / [`LogStore::needs_recovery`] until `recover()` re-reads the backing and
//! the caller decides what the durable state means for its own operation IDs.
//! Neither store retries for you: an append is not idempotent.
//!
//! # On-disk format
//!
//! Both file backings write **StructFS Value JSON v1** (the
//! `["structfs-value",1,…]` envelope from
//! [`structfs_serde_store::ValueJsonCodec`]) — a whole document for a
//! snapshot, one per line for a log. Before 0.5 they wrote plain
//! `serde_json`, which silently turns `Value::Bytes` into an array of numbers
//! and NaN/±inf into `null`.
//!
//! Reads accept both: a document is tried as the tagged form first and then
//! as plain JSON, so files written by 0.4 and earlier still load. The order
//! matters in one corner: a legacy document that is literally the array
//! `["structfs-value", 1, <valid tagged body>]` reads as the tagged form.
//! Writes are always tagged, so a file this version has written cannot be
//! read back by 0.4.
//!
//! Loads and saves are both bounded by the backing's
//! [`structfs_serde_store::Limits`] (the default unless set with
//! `with_limits`). A save or append the bounds reject fails before the file
//! is touched and leaves the store writable. The tagged form carries at most
//! 84 levels of nesting whatever the limits; a deeper legacy file loads but
//! cannot be saved again.

pub mod append_log;
pub mod persist;

pub use append_log::{AppendBacking, JsonlFileBacking, LogStore, MemoryAppendBacking};
pub use persist::{BackedStore, Backing, Durability, JsonFileBacking};
pub use structfs_core_store::{path, Error, Path, Reader, Record, Value, Writer};
