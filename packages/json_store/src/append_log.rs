//! Append-only logs: ledgers, usage records, event journals.
//!
//! Whole-file persistence ([`crate::persist::JsonFileBacking`]) rewrites
//! everything on every save — wrong for a ledger. An [`AppendBacking`]
//! appends one entry at a time (one line for the file form), and
//! [`LogStore`] serves the log as a store:
//!
//! | Path | Operation | Result |
//! |------|-----------|--------|
//! | `write append <value>` | Append an entry | Returns `entries/{n}` |
//! | `write append null` | — | `InvalidArgument` (see below) |
//! | `write <anything else>` | — | `PermissionDenied`: the log is append-only |
//! | `read ` (root) | All entries | `Value::Array` |
//! | `read len` | Entry count | `Value::Integer` |
//! | `read entries/{n}` | One entry | The entry, or `None` past the end |
//! | `read entries/{x}` | Non-numeric `{x}` | `InvalidArgument` |
//! | `read entries/from/{n}` | Tail from a cursor | `{items, next, status}` |
//! | `read entries/from/{x}` | Non-numeric `{x}` | `InvalidArgument` |
//! | `read <anything else>` | — | `None` |
//!
//! A malformed index is an argument error rather than a missing child: `1e3`
//! and `-1` are not names this store could ever have produced, so reporting
//! them as absent would hide a caller bug behind an empty read.
//!
//! **`Null` is rejected on append**, which makes the log the one place in this
//! crate where writing `Null` is not a deletion — deletion is meaningless for
//! an append-only log, and a stored `Null` entry would read back exactly like
//! an absent one, so it has no representation here.
//!
//! The tail read returns an `{items, next, status}` envelope, so consumers
//! page with a cursor instead of re-reading (and re-sorting) the whole
//! ledger. Logs have no
//! terminal state, so `status` is always `"open"`.
//!
//! # On-disk format
//!
//! [`JsonlFileBacking`] writes one **StructFS Value JSON v1** document per
//! line — the `["structfs-value",1,…]` envelope. Plain `serde_json` was used
//! before 0.5 and is lossy for `Value::Bytes` and non-finite floats; lines
//! written in that form are still read (each line is tried as the tagged form
//! first, then as plain JSON), but every new line is tagged, so a file that
//! has been appended to by 0.5 cannot be read by 0.4.
//!
//! The fallback is ordered, not sniffed: a legacy plain-JSON line that is
//! literally the array `["structfs-value", 1, <valid tagged body>]` is read
//! as the tagged form, i.e. as the value that body encodes.
//!
//! Both replay **and appends** are bounded by the backing's `Limits`
//! (`Limits::default()` unless replaced with
//! [`JsonlFileBacking::with_limits`]); each line is one document. An entry
//! the bounds reject fails before the file is touched, so the log stays
//! writable.

use std::io::Write as _;
use std::path::PathBuf;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};
use structfs_serde_store::Limits;

use crate::persist::{decode_document, encode_document, WriteStage};

/// Append-only persistence for a sequence of entries.
pub trait AppendBacking: Send + Sync {
    /// Load all previously appended entries, in order.
    fn load(&mut self) -> Result<Vec<Value>, Error>;

    /// Append at the backing's documented acknowledgement level. Errors may
    /// follow a partial or complete append; they do not imply rollback.
    fn append(&mut self, entry: &Value) -> Result<(), Error>;

    /// Whether the most recent failed [`AppendBacking::append`] may have
    /// changed the persisted log. [`LogStore`] fences further appends until
    /// `recover` only when this is `true`. The default is the conservative
    /// `true`; a backing that knows a failure preceded any write (an encoding
    /// error, say) should return `false`.
    fn last_failure_ambiguous(&self) -> bool {
        true
    }
}

/// JSON-lines file persistence: one entry per newline-terminated record.
/// Buffered mode acknowledges OS writes; Synced mode synchronizes the file and
/// parent directory (including file creation). Synced mode requires an existing
/// parent. Partial trailing records are rejected on load and before append;
/// repair requires an explicit operator decision, never silent truncation.
/// Use a single serialized writer. An error can leave an unacknowledged entry.
///
/// See the [module docs](self#on-disk-format) for the line format and for what
/// happens to files written by earlier versions.
pub struct JsonlFileBacking {
    path: PathBuf,
    durability: crate::persist::Durability,
    limits: Limits,
    /// Set once an append reaches the write into the log file; a failure
    /// before that point left the file untouched.
    writing: bool,
}

impl JsonlFileBacking {
    /// Persist to the given file path. The file need not exist yet;
    /// parent directories are created on first append.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            durability: crate::persist::Durability::Buffered,
            limits: Limits::default(),
            writing: false,
        }
    }

    /// Set the acknowledgement level. Defaults to `Durability::Buffered`.
    pub fn with_durability(mut self, durability: crate::persist::Durability) -> Self {
        self.durability = durability;
        self
    }

    /// Set the bounds applied to every replayed line and every append.
    /// Defaults to `Limits::default()`.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
}

impl AppendBacking for JsonlFileBacking {
    fn load(&mut self) -> Result<Vec<Value>, Error> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        if !text.is_empty() && !text.ends_with('\n') {
            return Err(Error::conflict(
                "incomplete JSONL tail; explicitly repair before reopening",
            ));
        }
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry = decode_document(line.as_bytes(), &self.limits).map_err(|e| {
                Error::decode(
                    structfs_core_store::Format::VALUE_JSON,
                    format!("bad log entry on line {}: {}", i + 1, e),
                )
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    fn append(&mut self, entry: &Value) -> Result<(), Error> {
        self.append_with(entry, |_| Ok(()))
    }

    /// Failures before the line is written (encoding, opening the file, a
    /// rejected partial tail) leave the log as it was.
    fn last_failure_ambiguous(&self) -> bool {
        self.writing
    }
}

/// In-memory backing: an ephemeral log (and the test double).
#[derive(Default)]
pub struct MemoryAppendBacking {
    entries: Vec<Value>,
}

impl MemoryAppendBacking {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AppendBacking for MemoryAppendBacking {
    fn load(&mut self) -> Result<Vec<Value>, Error> {
        Ok(self.entries.clone())
    }

    fn append(&mut self, entry: &Value) -> Result<(), Error> {
        self.entries.push(entry.clone());
        Ok(())
    }
}

/// An append-only log served as a store.
///
/// Entries are held in memory and appended through the backing before
/// the write returns. Everything except `append` is read-only.
pub struct LogStore<B: AppendBacking> {
    entries: Vec<Value>,
    backing: B,
    needs_recovery: bool,
}

impl<B: AppendBacking> LogStore<B> {
    /// Open a log, loading existing entries from the backing.
    pub fn open(mut backing: B) -> Result<Self, Error> {
        let entries = backing.load()?;
        Ok(Self {
            entries,
            backing,
            needs_recovery: false,
        })
    }

    /// Adopt readable backing state after an ambiguous append. This may include
    /// unacknowledged entries; callers must reconcile their own operation IDs.
    pub fn recover(&mut self) -> Result<(), Error> {
        self.needs_recovery = true;
        self.entries = self.backing.load()?;
        self.needs_recovery = false;
        Ok(())
    }
    pub fn needs_recovery(&self) -> bool {
        self.needs_recovery
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn tail_page(&self, from: usize) -> Value {
        let start = from.min(self.entries.len());
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "items".to_string(),
            Value::Array(self.entries[start..].to_vec()),
        );
        map.insert(
            "next".to_string(),
            Value::Integer(self.entries.len() as i64),
        );
        map.insert("status".to_string(), Value::from("open"));
        Value::Map(map)
    }
}

impl<B: AppendBacking> Reader for LogStore<B> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.is_empty() {
            return Ok(Some(Record::parsed(Value::Array(self.entries.clone()))));
        }
        let components: Vec<&str> = from.iter().collect();
        let value = match components.as_slice() {
            ["len"] => Some(Value::Integer(self.entries.len() as i64)),
            ["entries", "from", cursor] => {
                let cursor: usize = cursor.parse().map_err(|_| {
                    Error::invalid_argument(format!("log tail cursor must be a number: '{cursor}'"))
                })?;
                Some(self.tail_page(cursor))
            }
            ["entries", index] => {
                let index: usize = index.parse().map_err(|_| {
                    Error::invalid_argument(format!("log entry index must be a number: '{index}'"))
                })?;
                self.entries.get(index).cloned()
            }
            _ => None,
        };
        Ok(value.map(Record::parsed))
    }
}

impl<B: AppendBacking> Writer for LogStore<B> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() == 1 && &to[0] == "append" {
            let entry = data.into_value(&structfs_core_store::NoCodec)?;
            // The one place in this crate where Null is not a deletion: an
            // append-only log has nothing to delete, and a stored Null entry
            // would read back indistinguishably from an absent one.
            if entry.is_null() {
                return Err(Error::invalid_argument(
                    "cannot append Null: a log entry must be a value",
                ));
            }
            if self.needs_recovery {
                return Err(Error::conflict(
                    "append failed; recover backing before writing",
                ));
            }
            // Acknowledged before visible, at the backing's declared level.
            // Fenced for the duration of the append, then unfenced again if
            // the backing reports the failure preceded any write.
            self.needs_recovery = true;
            if let Err(error) = self.backing.append(&entry) {
                self.needs_recovery = self.backing.last_failure_ambiguous();
                return Err(error);
            }
            self.needs_recovery = false;
            let index = self.entries.len();
            self.entries.push(entry);
            return Ok(Path::from_components(vec![
                "entries".to_string(),
                index.to_string(),
            ]));
        }
        Err(Error::permission_denied(format!(
            "log is append-only: write to 'append', not '{}'",
            to
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    fn entry(n: i64) -> Record {
        Record::parsed(Value::Integer(n))
    }

    #[test]
    fn append_and_read_back() {
        let mut log = LogStore::open(MemoryAppendBacking::new()).unwrap();
        assert_eq!(
            log.write(&path!("append"), entry(1)).unwrap(),
            path!("entries/0")
        );
        assert_eq!(
            log.write(&path!("append"), entry(2)).unwrap(),
            path!("entries/1")
        );

        let all = log.read(&path!("")).unwrap().unwrap();
        assert!(matches!(all.as_value(), Some(Value::Array(a)) if a.len() == 2));
        assert_eq!(
            log.read(&path!("len")).unwrap().unwrap().as_value(),
            Some(&Value::Integer(2))
        );
        assert_eq!(
            log.read(&path!("entries/1")).unwrap().unwrap().as_value(),
            Some(&Value::Integer(2))
        );
        assert!(log.read(&path!("entries/9")).unwrap().is_none());
    }

    #[test]
    fn cursor_tail_pages_instead_of_rereading() {
        let mut log = LogStore::open(MemoryAppendBacking::new()).unwrap();
        for n in 0..5 {
            log.write(&path!("append"), entry(n)).unwrap();
        }

        let page = log.read(&path!("entries/from/3")).unwrap().unwrap();
        match page.as_value().unwrap() {
            Value::Map(map) => {
                assert!(matches!(map.get("items"), Some(Value::Array(a)) if a.len() == 2));
                assert_eq!(map.get("next"), Some(&Value::Integer(5)));
                assert_eq!(map.get("status"), Some(&Value::from("open")));
            }
            other => panic!("expected tail envelope, got {other:?}"),
        }

        // Stale cursors clamp.
        let page = log.read(&path!("entries/from/99")).unwrap().unwrap();
        assert!(matches!(
            page.as_value(),
            Some(Value::Map(map)) if matches!(map.get("items"), Some(Value::Array(a)) if a.is_empty())
        ));
    }

    #[test]
    fn non_append_writes_denied() {
        let mut log = LogStore::open(MemoryAppendBacking::new()).unwrap();
        let err = log.write(&path!("entries/0"), entry(9)).unwrap_err();
        assert!(matches!(err, Error::PermissionDenied { .. }));
    }

    fn nested(depth: usize) -> Value {
        let mut value = Value::from(1i64);
        for _ in 0..depth {
            value = Value::Array(vec![value]);
        }
        value
    }

    /// An entry the bounds reject fails before the file is touched: the log
    /// is unchanged and still accepts appends. Raising the bounds admits it.
    #[test]
    fn limits_bound_appends_and_replay_without_fencing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bounded.jsonl");
        let backing = |depth| {
            JsonlFileBacking::new(&file).with_limits(Limits::default().with_max_depth(depth))
        };

        let mut log = LogStore::open(backing(4)).unwrap();
        log.write(&path!("append"), entry(1)).unwrap();
        let before = std::fs::read(&file).unwrap();

        assert!(log
            .write(&path!("append"), Record::parsed(nested(10)))
            .is_err());
        assert!(!log.needs_recovery());
        assert_eq!(log.len(), 1);
        assert_eq!(std::fs::read(&file).unwrap(), before);
        log.write(&path!("append"), entry(2)).unwrap();

        let mut raised = LogStore::open(backing(64)).unwrap();
        raised
            .write(&path!("append"), Record::parsed(nested(10)))
            .unwrap();
        // The default-bounded replay of that deeper line needs the raised
        // bounds too.
        assert!(LogStore::open(backing(4)).is_err());
        assert_eq!(LogStore::open(backing(64)).unwrap().len(), 3);
    }

    #[test]
    fn appending_null_is_an_argument_error() {
        let mut log = LogStore::open(MemoryAppendBacking::new()).unwrap();
        let err = log
            .write(&path!("append"), Record::parsed(Value::Null))
            .unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument { .. }),
            "expected InvalidArgument, got {err:?}"
        );
        assert!(log.is_empty());
    }

    #[test]
    fn malformed_indices_are_argument_errors_not_absences() {
        let mut log = LogStore::open(MemoryAppendBacking::new()).unwrap();
        log.write(&path!("append"), entry(1)).unwrap();

        for bad in [path!("entries/nope"), path!("entries/from/nope")] {
            let err = log.read(&bad).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument { .. }),
                "expected InvalidArgument for {bad}, got {err:?}"
            );
        }

        // An index past the end is genuinely absent, not an error.
        assert!(log.read(&path!("entries/9")).unwrap().is_none());
        // Unknown paths are absent too.
        assert!(log.read(&path!("whatever")).unwrap().is_none());
    }

    /// The reason the line format changed: plain JSON cannot carry these.
    #[test]
    fn bytes_and_non_finite_floats_survive_append_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lossless.jsonl");

        let originals = vec![
            Value::Bytes(vec![0, 159, 146, 150]),
            Value::Float(f64::INFINITY),
            Value::Float(f64::NEG_INFINITY),
            Value::Unsigned(u64::MAX),
        ];
        {
            let mut log = LogStore::open(JsonlFileBacking::new(&file)).unwrap();
            for value in &originals {
                log.write(&path!("append"), Record::parsed(value.clone()))
                    .unwrap();
            }
            log.write(&path!("append"), Record::parsed(Value::Float(f64::NAN)))
                .unwrap();
        }

        let mut replayed = LogStore::open(JsonlFileBacking::new(&file)).unwrap();
        assert_eq!(replayed.len(), originals.len() + 1);
        for (i, expected) in originals.iter().enumerate() {
            let path = Path::from_components(vec!["entries".to_string(), i.to_string()]);
            assert_eq!(
                replayed.read(&path).unwrap().unwrap().as_value(),
                Some(expected)
            );
        }
        match replayed
            .read(&path!("entries/4"))
            .unwrap()
            .unwrap()
            .as_value()
        {
            Some(Value::Float(f)) => assert!(f.is_nan()),
            other => panic!("expected a NaN float, got {other:?}"),
        }
    }

    /// Lines written by 0.4 and earlier hold plain JSON; they still replay.
    #[test]
    fn legacy_plain_json_lines_still_replay() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("legacy.jsonl");
        std::fs::write(&file, b"1\n\"second entry\"\n{\"k\":true}\n").unwrap();

        let mut log = LogStore::open(JsonlFileBacking::new(&file)).unwrap();
        assert_eq!(log.len(), 3);
        assert_eq!(
            log.read(&path!("entries/1")).unwrap().unwrap().as_value(),
            Some(&Value::from("second entry"))
        );

        // New entries are appended in the tagged form, beside the old lines.
        log.write(&path!("append"), entry(4)).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 4);
        assert_eq!(text.lines().next().unwrap(), "1");
        assert_eq!(
            text.lines().last().unwrap(),
            "[\"structfs-value\",1,[\"int\",\"4\"]]"
        );
    }

    #[test]
    fn jsonl_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ledger.jsonl");

        {
            let mut log = LogStore::open(JsonlFileBacking::new(&file)).unwrap();
            log.write(&path!("append"), entry(1)).unwrap();
            log.write(
                &path!("append"),
                Record::parsed(Value::from("second entry")),
            )
            .unwrap();
        }

        let mut reopened = LogStore::open(JsonlFileBacking::new(&file)).unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened
                .read(&path!("entries/1"))
                .unwrap()
                .unwrap()
                .as_value(),
            Some(&Value::from("second entry"))
        );

        // The file really is one document per line, in the tagged form.
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(
            text.lines().next().unwrap(),
            "[\"structfs-value\",1,[\"int\",\"1\"]]"
        );
    }

    #[test]
    fn missing_jsonl_opens_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = LogStore::open(JsonlFileBacking::new(dir.path().join("nope.jsonl"))).unwrap();
        assert!(log.is_empty());
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::Durability;
    use structfs_core_store::path;
    #[test]
    fn partial_tail_is_never_silently_adopted_or_extended() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("log");
        let mut backing = JsonlFileBacking::new(&file).with_durability(Durability::Synced);
        backing.append(&Value::Integer(1)).unwrap();
        assert_eq!(backing.load().unwrap(), vec![Value::Integer(1)]);
        for tail in ["2", "{", "\"incomplete"] {
            std::fs::write(&file, format!("1\n{tail}")).unwrap();
            assert!(backing.load().is_err());
            assert!(backing.append(&Value::Null).is_err());
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                format!("1\n{tail}")
            );
        }
    }
    #[test]
    fn ambiguous_append_requires_reconciliation_without_duplicate_retry() {
        struct Ambiguous(Vec<Value>);
        impl AppendBacking for Ambiguous {
            fn load(&mut self) -> Result<Vec<Value>, Error> {
                Ok(self.0.clone())
            }
            fn append(&mut self, v: &Value) -> Result<(), Error> {
                self.0.push(v.clone());
                Err(Error::Io(std::io::Error::other("sync failed")))
            }
        }
        let mut log = LogStore::open(Ambiguous(vec![])).unwrap();
        assert!(log
            .write(&path!("append"), Record::parsed(Value::Integer(1)))
            .is_err());
        assert!(log.is_empty());
        assert!(log.needs_recovery());
        assert!(log
            .write(&path!("append"), Record::parsed(Value::Integer(1)))
            .is_err());
        log.recover().unwrap();
        assert_eq!(log.len(), 1);
    }
}

impl JsonlFileBacking {
    fn append_with(
        &mut self,
        entry: &Value,
        mut before: impl FnMut(WriteStage) -> std::io::Result<()>,
    ) -> Result<(), Error> {
        self.writing = false;
        // Encode first: an entry the limits reject fails before anything on
        // disk is created or opened.
        let line = encode_document(entry, &self.limits)?;
        if self.durability == crate::persist::Durability::Buffered {
            std::fs::create_dir_all(crate::persist::parent(&self.path))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.path)?;
        use std::io::{Read, Seek, SeekFrom};
        if file.metadata()?.len() > 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                return Err(Error::conflict(
                    "incomplete JSONL tail; explicitly repair before append",
                ));
            }
        }
        before(WriteStage::Write)?;
        self.writing = true;
        file.write_all(&line)?;
        writeln!(file)?;
        if self.durability == crate::persist::Durability::Synced {
            before(WriteStage::Sync)?;
            file.sync_all()?;
            before(WriteStage::DirectorySync)?;
            crate::persist::sync_parent(&self.path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod append_failures {
    use super::*;
    #[test]
    fn write_and_sync_errors_expose_the_actual_reopen_boundary() {
        for stage in [
            WriteStage::Write,
            WriteStage::Sync,
            WriteStage::DirectorySync,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let mut backing = JsonlFileBacking::new(dir.path().join("log"))
                .with_durability(crate::Durability::Synced);
            let mut order = Vec::new();
            assert!(backing
                .append_with(&Value::Integer(1), |point| {
                    order.push(point);
                    if point == stage {
                        Err(std::io::Error::other("injected"))
                    } else {
                        Ok(())
                    }
                })
                .is_err());
            assert_eq!(order.last(), Some(&stage));
            assert_eq!(
                backing.last_failure_ambiguous(),
                stage != WriteStage::Write,
                "ambiguity at {stage:?}"
            );
            assert_eq!(
                backing.load().unwrap().len(),
                if stage == WriteStage::Write { 0 } else { 1 }
            );
        }
    }
}
