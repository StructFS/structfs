//! The session log: assembly-wide forensics
//! ([spec 12](https://github.com/StructFS/structfs/blob/main/isotope/spec/12-determinism.md)).
//!
//! Per-block transcripts deliberately record no cross-block order —
//! each block's boundary is totally ordered by its own execution, and
//! replay must not depend on an interleaving the blocks never agreed
//! on. But the interleaving is exactly what a debugger's timeline view
//! and a post-incident investigation want. The session log is that
//! missing view: one arrival-order witness of every boundary operation
//! across every block, `{seq, block, op, path, outcome, entry?}` per
//! line, with `entry` linking into the block's transcript when one is
//! being kept (or consumed — a replay writes a session log too).
//!
//! **Observation-class, by construction.** The log is written after
//! each operation completes and answers nothing; replay never reads
//! it, and a recording is complete without it. A failure to append is
//! a warning, not a failed operation — forensics must never change
//! the run it observes. The `seq` order is the order appends reached
//! this log's lock: an honest witness of what happened, never a
//! contract about what must happen again.
//!
//! Like the transcript, the log is a store, written with the
//! append-log convention; the on-disk representation is the store's
//! business. It works in every mode: live (a flight recorder with no
//! transcript at all), recording (entries link into the transcripts),
//! and replay (the re-run's own timeline, comparable to the
//! original's).

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use structfs_core_store::{path, Error, Path, Record, Writer as _};
use structfs_serde_store::to_value;

use crate::namespace::HostStore;

/// One boundary operation, as the session witnessed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// Arrival order across the whole session, dense from 0.
    pub seq: u64,
    /// The block's transcript key (`root/block`, `root/nested/block`).
    pub block: String,
    pub op: String,
    pub path: Path,
    /// `found`, `absent`, `wrote`, or `failed:<kind>` — enough to read
    /// a timeline without joining; the full answer lives in the
    /// block's transcript at `entry`.
    pub outcome: String,
    /// Index into the block's transcript, when one is kept or
    /// consumed; absent in live mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<u64>,
}

struct SessionInner {
    seq: u64,
    log: HostStore,
}

/// The assembly-wide arrival-order log. One lock covers sequence
/// assignment and the append, so the log's physical order is the seq
/// order.
pub struct SessionLog {
    inner: Mutex<SessionInner>,
}

impl SessionLog {
    pub fn new(log: HostStore) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(SessionInner { seq: 0, log }),
        })
    }

    /// Witness one completed operation. Never fails the operation it
    /// observes: an append error is traced and dropped.
    pub(crate) fn witness(
        &self,
        block: &str,
        op: &str,
        at: &Path,
        outcome: String,
        entry: Option<u64>,
    ) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let record = SessionEntry {
            seq: inner.seq,
            block: block.to_string(),
            op: op.to_string(),
            path: at.clone(),
            outcome,
            entry,
        };
        let appended = to_value(&record)
            .and_then(|value| inner.log.write(&path!("append"), Record::parsed(value)));
        match appended {
            Ok(_) => inner.seq += 1,
            Err(error) => {
                tracing::warn!(block, %error, "session log append failed; entry dropped");
            }
        }
    }
}

/// The outcome label for a read result.
pub(crate) fn read_outcome(result: &Result<Option<Record>, Error>) -> String {
    match result {
        Ok(Some(_)) => "found".to_string(),
        Ok(None) => "absent".to_string(),
        Err(error) => failed(error),
    }
}

/// The outcome label for a write result.
pub(crate) fn write_outcome(result: &Result<Path, Error>) -> String {
    match result {
        Ok(_) => "wrote".to_string(),
        Err(error) => failed(error),
    }
}

fn failed(error: &Error) -> String {
    let kind = match error {
        Error::NotFound { .. } => "not_found",
        Error::NoRoute { .. } => "no_route",
        Error::PermissionDenied { .. } => "permission_denied",
        Error::Conflict { .. } => "conflict",
        Error::Overloaded { .. } => "overloaded",
        Error::DeadlineExceeded { .. } => "deadline_exceeded",
        Error::ResourceLimit { .. } => "resource_limit",
        Error::Cancelled { .. } => "cancelled",
        _ => "other",
    };
    format!("failed:{kind}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::host_store;
    use structfs_core_store::{Reader as _, Value};
    use structfs_json_store::{LogStore, MemoryAppendBacking};

    #[test]
    fn the_log_orders_and_labels_operations() {
        let store = host_store(LogStore::open(MemoryAppendBacking::new()).unwrap());
        let session = SessionLog::new(store.clone());
        session.witness(
            "demo/a",
            "read",
            &path!("iso/random/uuid"),
            "found".into(),
            Some(0),
        );
        session.witness("demo/b", "write", &path!("greeting"), "wrote".into(), None);
        session.witness(
            "demo/a",
            "read",
            &path!("secrets"),
            "failed:permission_denied".into(),
            Some(1),
        );

        let mut store = store;
        let all = store.read(&path!("")).unwrap().unwrap();
        let Some(Value::Array(items)) = all.as_value() else {
            panic!("expected the log's array");
        };
        assert_eq!(items.len(), 3);
        let entries: Vec<SessionEntry> = items
            .iter()
            .map(|item| structfs_serde_store::from_value(item.clone()).unwrap())
            .collect();
        assert_eq!(
            entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(entries[0].block, "demo/a");
        assert_eq!(entries[1].outcome, "wrote");
        assert_eq!(entries[2].outcome, "failed:permission_denied");
        assert_eq!(entries[2].entry, Some(1));
        assert_eq!(entries[1].entry, None);
    }

    #[test]
    fn outcome_labels_cover_the_answer_kinds() {
        assert_eq!(
            read_outcome(&Ok(Some(Record::parsed(Value::Null)))),
            "found"
        );
        assert_eq!(read_outcome(&Ok(None)), "absent");
        assert_eq!(
            read_outcome(&Err(Error::permission_denied("no"))),
            "failed:permission_denied"
        );
        assert_eq!(write_outcome(&Ok(path!("x"))), "wrote");
        assert_eq!(
            write_outcome(&Err(Error::conflict("diverged"))),
            "failed:conflict"
        );
    }
}
