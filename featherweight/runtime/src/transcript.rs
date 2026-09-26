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
//! obligations apply.
//!
//! Transcripts are keyed by an assembly-scoped path — `demo/shell`,
//! `demo/sub/inner` for a nested assembly's block, with `#2`, `#3`
//! appended when one key recurs (the same definition spawned twice).
//! Blocks spawned dynamically through `iso/proc` go through the same
//! start path as static ones and are transcribed like them; on replay a
//! spawner's spawn write is answered from its transcript without
//! re-executing, so children do not run — each child's own transcript
//! replays it separately.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use structfs_core_store::{path, CodecDiagnostic, Error, Path, Record, Value};
use structfs_serde_store::{from_value, to_value};

use crate::namespace::HostStore;
use crate::protocol::{ErrorKind, ErrorParts};

/// Where a block's transcript lives, by block name. The store must honor the
/// append-log convention (`write append`, `read entries/from/{n}`).
pub type TranscriptProvider = dyn Fn(&str) -> std::result::Result<HostStore, Error> + Send + Sync;

/// The runtime's transcript mode (spec 12: a mode, not a mandate).
#[derive(Clone, Default)]
#[non_exhaustive]
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
    /// Replay each block's transcript *prefix*, then hand off to live
    /// execution: the block wakes at an arbitrary recorded state and
    /// keeps running against the real world. `to(key)` gives the number
    /// of entries to replay for a block (`None` = its whole transcript).
    ///
    /// Sound only for a prefix whose effects stay inside `/iso`: an
    /// effect into a wired peer was suppressed during replay, so the
    /// live world would be missing state the block believes in — such
    /// a seek is refused at start, naming the offending entry. Seeded
    /// determinism sources are fast-forwarded past the prefix, so a
    /// seeded seek continues exactly where a straight run would be.
    /// Payload-carrying `/iso` metadata (a declared interface, armed
    /// timers) is not reconstructed — payloads are deliberately not on
    /// the transcript.
    Seek {
        provider: Arc<TranscriptProvider>,
        to: Arc<SeekPoint>,
    },
}

/// Per-block seek horizon: how many transcript entries to replay before
/// handing off (`None` = the whole transcript). Blocks that had not run
/// by the sought point get `Some(0)` — they start live from the top.
pub type SeekPoint = dyn Fn(&str) -> Option<u64> + Send + Sync;

/// One boundary operation and its complete answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TranscriptEntry {
    pub op: String,
    pub path: Path,
    pub answer: TranscriptAnswer,
    /// For writes: a digest of the payload the block wrote. Replay
    /// checks it, so a run that writes *different data* to the same
    /// path is caught as divergence, not silently acknowledged — the
    /// payload itself stays off the transcript (outputs can be bulky;
    /// the digest is enough to catch the lie).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrote: Option<String>,
}

/// The write-payload digest: fnv1a-64 over the record's canonical JSON,
/// hex. Not cryptographic — it detects divergence, it doesn't defend
/// against an adversary who owns the transcript anyway.
pub(crate) fn digest(data: &Record) -> Option<String> {
    let rendered = serde_json::to_string(data).ok()?;
    Some(format!("{:016x}", crate::hash::fnv1a(rendered.as_bytes())))
}

/// What the world said. `Found`/`Absent` answer reads, `Wrote` answers
/// writes, and `Failed` answers either — refusals are answers too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TranscriptAnswer {
    Found(Record),
    Absent,
    Wrote(Path),
    Failed(TranscriptError),
}

/// An error as the transcript holds it: the typed kind (so replayed code
/// branches the same way — see [`ErrorKind::label`]) plus the error's own
/// message and whatever structured detail typed reconstruction needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TranscriptError {
    pub kind: String,
    pub message: String,
    /// `not_found` / `no_route`: the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Path>,
    /// `invalid_path`: the offending component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// `invalid_path`: the component's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u64>,
    /// `codec`: the portable codec diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<CodecDiagnostic>,
}

fn error_to_transcript(error: &Error) -> TranscriptError {
    let parts = ErrorParts::of(error);
    let (component, position) = match parts.component {
        Some((component, position)) => (Some(component), Some(position)),
        None => (None, None),
    };
    TranscriptError {
        kind: parts.kind.label().to_string(),
        message: parts.message,
        path: parts.path,
        component,
        position,
        codec: parts.codec,
    }
}

fn transcript_to_error(transcript: TranscriptError) -> Error {
    ErrorParts {
        kind: ErrorKind::from_label(&transcript.kind),
        message: transcript.message,
        path: transcript.path,
        component: transcript
            .component
            .map(|component| (component, transcript.position.unwrap_or(0))),
        codec: transcript.codec,
    }
    .into_error("transcript", "replayed")
}

/// One block's transcript, held by its [`crate::Namespace`].
pub(crate) enum BlockTranscript {
    Recording {
        log: HostStore,
        /// Entries appended so far — the next entry's index.
        appended: u64,
    },
    Replaying {
        entries: VecDeque<TranscriptEntry>,
        /// Entries consumed so far — the index divergence errors cite.
        cursor: usize,
        /// Seek mode: hand off to live execution when the entries run
        /// out, instead of failing the run.
        then_live: bool,
    },
    /// A seek that reached its horizon: the block runs live, the
    /// transcript inert.
    HandedOff,
}

/// What a seek's replayed prefix consumed and touched — computed before
/// the block runs, to fast-forward seeded sources and to refuse unsound
/// handoffs.
pub(crate) struct PreambleProfile {
    /// splitmix64 words the prefix drew from `/iso/random`.
    pub(crate) entropy_words: u64,
    /// Virtual-clock ticks the prefix consumed from `/iso/time`.
    pub(crate) clock_ticks: u64,
    /// The first effect into a wired (non-iso) target, with its entry
    /// index — present means the handoff is unsound and must be refused.
    pub(crate) peer_write: Option<(u64, Path)>,
}

impl BlockTranscript {
    pub(crate) fn recording(log: HostStore) -> Self {
        BlockTranscript::Recording { log, appended: 0 }
    }

    /// The index the next operation will occupy (recording) or consume
    /// (replaying) — what a session-log entry links to. `None` once a
    /// seek has handed off: live operations have no transcript entry.
    pub(crate) fn position(&self) -> Option<u64> {
        match self {
            BlockTranscript::Recording { appended, .. } => Some(*appended),
            BlockTranscript::Replaying { cursor, .. } => Some(*cursor as u64),
            BlockTranscript::HandedOff => None,
        }
    }

    /// Whether a seek's replay has reached its horizon and the next
    /// operation should run live. The caller flips the transcript to
    /// [`BlockTranscript::HandedOff`] when this answers true.
    pub(crate) fn handoff_ready(&self) -> bool {
        matches!(
            self,
            BlockTranscript::Replaying {
                entries,
                then_live: true,
                ..
            } if entries.is_empty()
        )
    }

    /// Load a transcript prefix for seek-then-live: replay `to` entries
    /// (`None` = all of them), then hand off instead of failing.
    pub(crate) async fn seeking(log: HostStore, to: Option<u64>) -> Result<Self, Error> {
        let mut loaded = Self::replaying(log).await?;
        let BlockTranscript::Replaying {
            entries, then_live, ..
        } = &mut loaded
        else {
            unreachable!("replaying() builds Replaying");
        };
        if let Some(horizon) = to {
            entries.truncate(horizon as usize);
        }
        *then_live = true;
        Ok(loaded)
    }

    /// What the (possibly truncated) prefix consumed and touched.
    pub(crate) fn preamble_profile(&self) -> PreambleProfile {
        let mut profile = PreambleProfile {
            entropy_words: 0,
            clock_ticks: 0,
            peer_write: None,
        };
        let BlockTranscript::Replaying { entries, .. } = self else {
            return profile;
        };
        for (index, entry) in entries.iter().enumerate() {
            let components: Vec<&str> = entry.path.iter().collect();
            let found = matches!(entry.answer, TranscriptAnswer::Found(_));
            match (entry.op.as_str(), components.as_slice()) {
                ("read", ["iso", "random", "uuid"]) if found => profile.entropy_words += 2,
                ("read", ["iso", "random", "int"]) if found => profile.entropy_words += 1,
                ("read", ["iso", "random", "bytes", n]) if found => {
                    let n: u64 = n.parse().unwrap_or(0);
                    profile.entropy_words += n.div_ceil(8);
                }
                ("read", ["iso", "time", "now" | "now_unix_ns" | "monotonic"]) if found => {
                    profile.clock_ticks += 1;
                }
                ("write", [first, ..]) if *first != "iso" && profile.peer_write.is_none() => {
                    profile.peer_write = Some((index as u64, entry.path.clone()));
                }
                _ => {}
            }
        }
        profile
    }

    /// Load a transcript for replay through the append-log tail convention.
    pub(crate) async fn replaying(log: HostStore) -> Result<Self, Error> {
        let page = log.read(&path!("entries/from/0")).await?.ok_or_else(|| {
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
        Ok(BlockTranscript::Replaying {
            entries,
            cursor: 0,
            then_live: false,
        })
    }

    pub(crate) fn is_replaying(&self) -> bool {
        matches!(self, BlockTranscript::Replaying { .. })
    }

    /// Entries the replay has not consumed. A replayed block that
    /// finishes with entries remaining stopped short of the recorded
    /// run — worth a warning, though not an error: a block may
    /// legitimately exit early on a replayed shutdown request.
    pub(crate) fn remaining(&self) -> usize {
        match self {
            BlockTranscript::Recording { .. } | BlockTranscript::HandedOff => 0,
            BlockTranscript::Replaying { entries, .. } => entries.len(),
        }
    }

    /// Append one answered operation. Failing to record fails the
    /// operation: a transcript with a hole is worse than a failed run.
    async fn record(
        &mut self,
        op: &str,
        at: &Path,
        answer: TranscriptAnswer,
        wrote: Option<String>,
    ) -> Result<(), Error> {
        let BlockTranscript::Recording { log, appended } = self else {
            return Ok(());
        };
        let entry = TranscriptEntry {
            op: op.to_string(),
            path: at.clone(),
            answer,
            wrote,
        };
        log.write(&path!("append"), Record::parsed(to_value(&entry)?))
            .await?;
        *appended += 1;
        Ok(())
    }

    pub(crate) async fn record_read(
        &mut self,
        at: &Path,
        result: &Result<Option<Record>, Error>,
    ) -> Result<(), Error> {
        let answer = match result {
            Ok(Some(record)) => TranscriptAnswer::Found(record.clone()),
            Ok(None) => TranscriptAnswer::Absent,
            Err(error) => TranscriptAnswer::Failed(error_to_transcript(error)),
        };
        self.record("read", at, answer, None).await
    }

    pub(crate) async fn record_write(
        &mut self,
        at: &Path,
        wrote: Option<String>,
        result: &Result<Path, Error>,
    ) -> Result<(), Error> {
        let answer = match result {
            Ok(path) => TranscriptAnswer::Wrote(path.clone()),
            Err(error) => TranscriptAnswer::Failed(error_to_transcript(error)),
        };
        self.record("write", at, answer, wrote).await
    }

    /// The next entry, checked against what the block actually asked.
    fn next(&mut self, op: &str, at: &Path) -> Result<TranscriptEntry, Error> {
        let BlockTranscript::Replaying {
            entries, cursor, ..
        } = self
        else {
            return Err(Error::store("transcript", "replay", "not replaying"));
        };
        let index = *cursor;
        let Some(entry) = entries.pop_front() else {
            return Err(Error::conflict(format!(
                "the transcript ran out at entry {index}, {op} {at} — the replayed \
                 run asked more of the world than the recorded one did"
            )));
        };
        *cursor += 1;
        if entry.op != op || entry.path != *at {
            return Err(Error::conflict(format!(
                "replay diverged at entry {index}: the transcript recorded {} {}, \
                 the run asked {op} {at} — the block was not a function of its inputs",
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

    pub(crate) fn replay_write(&mut self, at: &Path, wrote: Option<String>) -> Result<Path, Error> {
        let entry = self.next("write", at)?;
        // A write to the right path with the wrong payload is still a
        // diverged run: the acknowledgement would be a lie about data
        // the recorded world never saw.
        if let (Some(recorded), Some(actual)) = (&entry.wrote, &wrote) {
            if recorded != actual {
                return Err(Error::conflict(format!(
                    "replay diverged at write {at}: the run wrote different data \
                     than the recorded run did (digest {actual}, recorded {recorded})"
                )));
            }
        }
        match entry.answer {
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
            wrote: None,
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
        for error in crate::protocol::tests::every_error() {
            let replayed = transcript_to_error(error_to_transcript(&error));
            assert_eq!(
                ErrorKind::of(&replayed),
                ErrorKind::of(&error),
                "{error} came back as {replayed}"
            );
            // The entry survives the store's value form, too.
            let entry = to_value(&error_to_transcript(&error)).unwrap();
            assert_eq!(
                from_value::<TranscriptError>(entry).unwrap(),
                error_to_transcript(&error)
            );
        }
        // Untyped errors keep their rendered message.
        let other = Error::store("http", "read", "boom");
        let replayed = transcript_to_error(error_to_transcript(&other));
        assert!(replayed.to_string().contains("http::read: boom"));
    }

    #[tokio::test]
    async fn record_then_replay_answers_in_order() {
        let log = host_store(LogStore::open(MemoryAppendBacking::new()).unwrap());
        let mut transcript = BlockTranscript::recording(log.clone());
        transcript
            .record_read(
                &path!("iso/random/uuid"),
                &Ok(Some(Record::parsed(Value::from("u-1")))),
            )
            .await
            .unwrap();
        transcript
            .record_write(
                &path!("services/kv/greeting"),
                digest(&Record::parsed(Value::from("hello"))),
                &Ok(path!("greeting")),
            )
            .await
            .unwrap();
        transcript
            .record_read(
                &path!("services/nothing"),
                &Err(Error::permission_denied("unwired")),
            )
            .await
            .unwrap();

        let mut replay = BlockTranscript::replaying(log.clone()).await.unwrap();
        let answer = replay.replay_read(&path!("iso/random/uuid")).unwrap();
        assert_eq!(answer.unwrap().as_value(), Some(&Value::from("u-1")));
        assert_eq!(
            replay
                .replay_write(
                    &path!("services/kv/greeting"),
                    digest(&Record::parsed(Value::from("hello"))),
                )
                .unwrap(),
            path!("greeting")
        );
        assert!(matches!(
            replay.replay_read(&path!("services/nothing")),
            Err(Error::PermissionDenied { .. })
        ));
        assert_eq!(replay.remaining(), 0);

        // The same path with different data is a diverged run, not an
        // acknowledged write.
        let mut replay = BlockTranscript::replaying(log).await.unwrap();
        replay.replay_read(&path!("iso/random/uuid")).unwrap();
        let lied = replay
            .replay_write(
                &path!("services/kv/greeting"),
                digest(&Record::parsed(Value::from("goodbye"))),
            )
            .unwrap_err();
        assert!(lied.to_string().contains("different data"), "{lied}");
    }

    #[tokio::test]
    async fn divergence_and_exhaustion_fail_loudly() {
        let log = host_store(LogStore::open(MemoryAppendBacking::new()).unwrap());
        let mut transcript = BlockTranscript::recording(log.clone());
        transcript
            .record_read(&path!("iso/time/now"), &Ok(None))
            .await
            .unwrap();

        let mut replay = BlockTranscript::replaying(log.clone()).await.unwrap();
        let diverged = replay.replay_read(&path!("iso/random/uuid")).unwrap_err();
        assert!(diverged.to_string().contains("diverged"), "{diverged}");

        let mut replay = BlockTranscript::replaying(log).await.unwrap();
        replay.replay_read(&path!("iso/time/now")).unwrap();
        let out = replay.replay_read(&path!("iso/time/now")).unwrap_err();
        assert!(out.to_string().contains("ran out"), "{out}");
    }
}
