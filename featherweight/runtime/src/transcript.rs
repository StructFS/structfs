//! Per-block transcripts
//! ([spec 12](https://github.com/StructFS/structfs/blob/main/isotope/spec/12-determinism.md)).
//!
//! A **transcript** is the ordered record of every answer a block's
//! boundary gave — a block is single-threaded, so its namespace
//! operations are totally ordered and the transcript is a plain
//! sequence. Recording appends each `(op, path, answer)` after the
//! operation executes; replaying answers every operation from the
//! transcript and never touches the iso surface or the wiring at all —
//! the transcript is the world.
//!
//! Transcripts and determinism are orthogonal. Any run can be
//! transcribed — a live, nondeterministic one included, purely as a
//! record of what the world said. Determinism is the separate property
//! (spec 12) under which replaying a transcript reproduces the run;
//! replay is also how its absence is *detected*, as a divergence error
//! at the first question the replayed run asks differently.
//!
//! Three rules keep a transcript correct (spec 12, *Recording*):
//!
//! - **A refusal is an answer.** Errors — including the
//!   `PermissionDenied` an unwired path earns — are recorded and
//!   replayed, because blocks branch on them.
//! - **Write acknowledgements are inputs.** The result path of a write
//!   is recorded and replayed; the write's *effect* is never
//!   re-executed.
//! - **The path is recorded beside the answer.** On replay it is the
//!   divergence check: a block that asks a different question than the
//!   recorded run did was not a function of its inputs, and the replay
//!   fails loudly at that entry rather than quietly going elsewhere.
//!
//! The transcript is a store, not a file. Recording writes entries with the
//! append-log convention (`write append` → `entries/{n}`) and replaying
//! reads them back through the `entries/from/{0}` tail envelope, so any
//! store honoring that convention can hold a transcript — `structfs-json-store`'s
//! `LogStore` over a JSONL file (what the `fw` CLI mounts), an in-memory
//! log (what the tests use), or anything else. The on-disk
//! representation is the store's business, never the runtime's.
//!
//! Transcription is a mode, not a mandate: with
//! [`TranscriptMode::Off`] (the default) nothing here runs and no
//! obligations apply. Known strawman limits: transcripts are keyed by block
//! name (unique names per recording directory are the embedder's job),
//! and blocks created dynamically through `iso/proc` spawn run live —
//! the spawner's boundary is transcribed, the children's are not.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use structfs_core_store::{path, Error, Path, Reader, Record, Value, Writer};
use structfs_serde_store::{from_value, to_value};

use crate::namespace::HostStore;

/// Where a block's transcript lives, by block name. The store must honor the
/// append-log convention (`write append`, `read entries/from/{n}`).
pub type TranscriptProvider = dyn Fn(&str) -> std::result::Result<HostStore, Error> + Send + Sync;

/// The runtime's transcript mode (spec 12: a mode, not a mandate).
#[derive(Clone, Default)]
pub enum TranscriptMode {
    /// No transcripts; every operation runs against the live world.
    #[default]
    Off,
    /// Execute live, appending every boundary answer to each block's
    /// transcript store.
    Record(Arc<TranscriptProvider>),
    /// Answer every boundary operation from each block's transcript store;
    /// the live world is never consulted and effects are not re-executed.
    Replay(Arc<TranscriptProvider>),
}

/// One boundary operation and its complete answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub op: String,
    pub path: Path,
    pub answer: TranscriptAnswer,
}

/// What the world said. `Found`/`Absent` answer reads, `Wrote` answers
/// writes, and `Failed` answers either — refusals are answers too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptAnswer {
    Found(Record),
    Absent,
    Wrote(Path),
    Failed(TranscriptError),
}

/// An error as the transcript holds it: the typed kind (so replayed code
/// branches the same way) plus the rendered message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptError {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,
}

fn error_to_transcript(error: &Error) -> TranscriptError {
    let (kind, message, path) = match error {
        Error::NotFound { path } => ("not_found", String::new(), Some(path.clone())),
        Error::NoRoute { path } => ("no_route", String::new(), Some(path.clone())),
        Error::PermissionDenied { message } => ("permission_denied", message.clone(), None),
        Error::Conflict { message } => ("conflict", message.clone(), None),
        Error::Overloaded { message } => ("overloaded", message.clone(), None),
        Error::DeadlineExceeded { message } => ("deadline_exceeded", message.clone(), None),
        Error::ResourceLimit { message } => ("resource_limit", message.clone(), None),
        Error::Cancelled { message } => ("cancelled", message.clone(), None),
        other => ("other", other.to_string(), None),
    };
    TranscriptError {
        kind: kind.to_string(),
        message,
        path,
    }
}

fn transcript_to_error(transcript: TranscriptError) -> Error {
    match (transcript.kind.as_str(), transcript.path) {
        ("not_found", Some(path)) => Error::not_found(path),
        ("no_route", Some(path)) => Error::NoRoute { path },
        ("permission_denied", _) => Error::permission_denied(transcript.message),
        ("conflict", _) => Error::conflict(transcript.message),
        ("overloaded", _) => Error::overloaded(transcript.message),
        ("deadline_exceeded", _) => Error::deadline_exceeded(transcript.message),
        ("resource_limit", _) => Error::resource_limit(transcript.message),
        ("cancelled", _) => Error::cancelled(transcript.message),
        _ => Error::store("transcript", "replayed", transcript.message),
    }
}

/// One block's transcript, held by its [`crate::Namespace`].
pub(crate) enum BlockTranscript {
    Recording { log: HostStore },
    Replaying { entries: VecDeque<TranscriptEntry> },
}

impl BlockTranscript {
    pub(crate) fn recording(log: HostStore) -> Self {
        BlockTranscript::Recording { log }
    }

    /// Load a transcript for replay through the append-log tail convention.
    pub(crate) fn replaying(log: HostStore) -> Result<Self, Error> {
        let mut log = log;
        let page = log.read(&path!("entries/from/0"))?.ok_or_else(|| {
            Error::store(
                "transcript",
                "replay",
                "transcript store has no entries path",
            )
        })?;
        let items = match page.into_value(&structfs_core_store::NoCodec)? {
            Value::Map(mut map) => match map.remove("items") {
                Some(Value::Array(items)) => items,
                _ => {
                    return Err(Error::store(
                        "transcript",
                        "replay",
                        "transcript store's tail page has no items array",
                    ))
                }
            },
            _ => {
                return Err(Error::store(
                    "transcript",
                    "replay",
                    "transcript store did not answer with a tail page",
                ))
            }
        };
        let mut entries = VecDeque::with_capacity(items.len());
        for item in items {
            entries.push_back(from_value::<TranscriptEntry>(item)?);
        }
        Ok(BlockTranscript::Replaying { entries })
    }

    pub(crate) fn is_replaying(&self) -> bool {
        matches!(self, BlockTranscript::Replaying { .. })
    }

    /// Append one answered operation. Failing to record fails the
    /// operation: a transcript with a hole is worse than a failed run.
    fn record(&mut self, op: &str, at: &Path, answer: TranscriptAnswer) -> Result<(), Error> {
        let BlockTranscript::Recording { log } = self else {
            return Ok(());
        };
        let entry = TranscriptEntry {
            op: op.to_string(),
            path: at.clone(),
            answer,
        };
        log.write(&path!("append"), Record::parsed(to_value(&entry)?))?;
        Ok(())
    }

    pub(crate) fn record_read(
        &mut self,
        at: &Path,
        result: &Result<Option<Record>, Error>,
    ) -> Result<(), Error> {
        let answer = match result {
            Ok(Some(record)) => TranscriptAnswer::Found(record.clone()),
            Ok(None) => TranscriptAnswer::Absent,
            Err(error) => TranscriptAnswer::Failed(error_to_transcript(error)),
        };
        self.record("read", at, answer)
    }

    pub(crate) fn record_write(
        &mut self,
        at: &Path,
        result: &Result<Path, Error>,
    ) -> Result<(), Error> {
        let answer = match result {
            Ok(path) => TranscriptAnswer::Wrote(path.clone()),
            Err(error) => TranscriptAnswer::Failed(error_to_transcript(error)),
        };
        self.record("write", at, answer)
    }

    /// The next entry, checked against what the block actually asked.
    fn next(&mut self, op: &str, at: &Path) -> Result<TranscriptEntry, Error> {
        let BlockTranscript::Replaying { entries } = self else {
            return Err(Error::store("transcript", "replay", "not replaying"));
        };
        let Some(entry) = entries.pop_front() else {
            return Err(Error::conflict(format!(
                "the transcript ran out at {op} {at} — the replayed run asked more \
                 of the world than the recorded one did"
            )));
        };
        if entry.op != op || entry.path != *at {
            return Err(Error::conflict(format!(
                "replay diverged: the transcript recorded {} {}, the run asked {op} {at} \
                 — the block was not a function of its inputs",
                entry.op, entry.path
            )));
        }
        Ok(entry)
    }

    pub(crate) fn replay_read(&mut self, at: &Path) -> Result<Option<Record>, Error> {
        match self.next("read", at)?.answer {
            TranscriptAnswer::Found(record) => Ok(Some(record)),
            TranscriptAnswer::Absent => Ok(None),
            TranscriptAnswer::Failed(error) => Err(transcript_to_error(error)),
            TranscriptAnswer::Wrote(_) => Err(Error::conflict(format!(
                "the transcript holds a write acknowledgement where the run read {at}"
            ))),
        }
    }

    pub(crate) fn replay_write(&mut self, at: &Path) -> Result<Path, Error> {
        match self.next("write", at)?.answer {
            TranscriptAnswer::Wrote(path) => Ok(path),
            TranscriptAnswer::Failed(error) => Err(transcript_to_error(error)),
            TranscriptAnswer::Found(_) | TranscriptAnswer::Absent => Err(Error::conflict(format!(
                "the transcript holds a read answer where the run wrote {at}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::namespace::host_store;
    use structfs_json_store::{LogStore, MemoryAppendBacking};

    fn entries_round_trip(answer: TranscriptAnswer) {
        let entry = TranscriptEntry {
            op: "read".to_string(),
            path: path!("iso/time/now"),
            answer,
        };
        let value = to_value(&entry).unwrap();
        let reread = from_value::<TranscriptEntry>(value.clone()).unwrap();
        // Compare through the store's own currency: Value is Eq, Record
        // is not.
        assert_eq!(to_value(&reread).unwrap(), value);
    }

    #[test]
    fn every_answer_kind_survives_the_store() {
        entries_round_trip(TranscriptAnswer::Found(Record::parsed(Value::from(
            "hello",
        ))));
        entries_round_trip(TranscriptAnswer::Absent);
        entries_round_trip(TranscriptAnswer::Wrote(path!("entries/3")));
        entries_round_trip(TranscriptAnswer::Failed(error_to_transcript(
            &Error::permission_denied("unwired"),
        )));
    }

    #[test]
    fn typed_errors_replay_typed() {
        for error in [
            Error::not_found(path!("users/nobody")),
            Error::permission_denied("unwired"),
            Error::conflict("stale"),
            Error::overloaded("busy"),
            Error::deadline_exceeded("30s"),
            Error::resource_limit("quota"),
            Error::cancelled("shutdown"),
        ] {
            let replayed = transcript_to_error(error_to_transcript(&error));
            assert_eq!(
                std::mem::discriminant(&replayed),
                std::mem::discriminant(&error),
                "{error} came back as {replayed}"
            );
        }
        // Untyped errors keep their rendered message.
        let other = Error::store("http", "read", "boom");
        let replayed = transcript_to_error(error_to_transcript(&other));
        assert!(replayed.to_string().contains("http::read: boom"));
    }

    #[test]
    fn record_then_replay_answers_in_order() {
        let log = host_store(LogStore::open(MemoryAppendBacking::new()).unwrap());
        let mut transcript = BlockTranscript::recording(log.clone());
        transcript
            .record_read(
                &path!("iso/random/uuid"),
                &Ok(Some(Record::parsed(Value::from("u-1")))),
            )
            .unwrap();
        transcript
            .record_write(&path!("services/kv/greeting"), &Ok(path!("greeting")))
            .unwrap();
        transcript
            .record_read(
                &path!("services/nothing"),
                &Err(Error::permission_denied("unwired")),
            )
            .unwrap();

        let mut replay = BlockTranscript::replaying(log).unwrap();
        let answer = replay.replay_read(&path!("iso/random/uuid")).unwrap();
        assert_eq!(answer.unwrap().as_value(), Some(&Value::from("u-1")));
        assert_eq!(
            replay.replay_write(&path!("services/kv/greeting")).unwrap(),
            path!("greeting")
        );
        assert!(matches!(
            replay.replay_read(&path!("services/nothing")),
            Err(Error::PermissionDenied { .. })
        ));
    }

    #[test]
    fn divergence_and_exhaustion_fail_loudly() {
        let log = host_store(LogStore::open(MemoryAppendBacking::new()).unwrap());
        let mut transcript = BlockTranscript::recording(log.clone());
        transcript
            .record_read(&path!("iso/time/now"), &Ok(None))
            .unwrap();

        let mut replay = BlockTranscript::replaying(log.clone()).unwrap();
        let diverged = replay.replay_read(&path!("iso/random/uuid")).unwrap_err();
        assert!(diverged.to_string().contains("diverged"), "{diverged}");

        let mut replay = BlockTranscript::replaying(log).unwrap();
        replay.replay_read(&path!("iso/time/now")).unwrap();
        let out = replay.replay_read(&path!("iso/time/now")).unwrap_err();
        assert!(out.to_string().contains("ran out"), "{out}");
    }
}
