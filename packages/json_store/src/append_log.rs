//! Append-only logs: ledgers, usage records, event journals.
//!
//! Whole-file persistence ([`crate::persist::JsonFileBacking`]) rewrites
//! everything on every save — wrong for a ledger. An [`AppendBacking`]
//! appends one entry at a time (one JSON line for the file form), and
//! [`LogStore`] serves the log as a store:
//!
//! | Path | Operation | Result |
//! |------|-----------|--------|
//! | `write /append <value>` | Append an entry | Returns `entries/{n}` |
//! | `read /` | All entries | `Value::Array` |
//! | `read /len` | Entry count | `Value::Integer` |
//! | `read /entries/{n}` | One entry | The entry |
//! | `read /entries/from/{n}` | Tail from a cursor | `{items, next, status}` |
//!
//! The tail read returns the same `{items, next, status}` envelope as
//! `structfs-handles`' `TailPage`, so consumers page with a cursor
//! instead of re-reading (and re-sorting) the whole ledger. Logs have no
//! terminal state, so `status` is always `"open"`.

use std::io::Write as _;
use std::path::PathBuf;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// Append-only persistence for a sequence of entries.
pub trait AppendBacking: Send + Sync {
    /// Load all previously appended entries, in order.
    fn load(&mut self) -> Result<Vec<Value>, Error>;

    /// Durably append one entry.
    fn append(&mut self, entry: &Value) -> Result<(), Error>;
}

/// JSON-lines file persistence: one entry per line, appended in place.
pub struct JsonlFileBacking {
    path: PathBuf,
}

impl JsonlFileBacking {
    /// Persist to the given file path. The file need not exist yet;
    /// parent directories are created on first append.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl AppendBacking for JsonlFileBacking {
    fn load(&mut self) -> Result<Vec<Value>, Error> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::Io(e)),
        };
        let mut entries = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: Value = serde_json::from_str(line).map_err(|e| {
                Error::decode(
                    structfs_core_store::Format::JSON,
                    format!("bad JSONL entry on line {}: {}", i + 1, e),
                )
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    fn append(&mut self, entry: &Value) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let line = serde_json::to_string(entry)
            .map_err(|e| Error::encode(structfs_core_store::Format::JSON, e.to_string()))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        Ok(())
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
}

impl<B: AppendBacking> LogStore<B> {
    /// Open a log, loading existing entries from the backing.
    pub fn open(mut backing: B) -> Result<Self, Error> {
        let entries = backing.load()?;
        Ok(Self { entries, backing })
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
        let components: Vec<&str> = from.iter().map(String::as_str).collect();
        let value = match components.as_slice() {
            ["len"] => Some(Value::Integer(self.entries.len() as i64)),
            ["entries", "from", cursor] => {
                let cursor: usize = cursor
                    .parse()
                    .map_err(|_| Error::store("log", "read", "bad cursor"))?;
                Some(self.tail_page(cursor))
            }
            ["entries", index] => {
                let index: usize = index
                    .parse()
                    .map_err(|_| Error::store("log", "read", "bad entry index"))?;
                self.entries.get(index).cloned()
            }
            _ => None,
        };
        Ok(value.map(Record::parsed))
    }
}

impl<B: AppendBacking> Writer for LogStore<B> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() == 1 && to[0] == "append" {
            let entry = data.into_value(&structfs_core_store::NoCodec)?;
            // Durable before visible: the backing accepts the entry
            // before it appears in reads.
            self.backing.append(&entry)?;
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

        // The file really is JSON lines.
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert_eq!(text.lines().next().unwrap(), "1");
    }

    #[test]
    fn missing_jsonl_opens_empty() {
        let dir = tempfile::tempdir().unwrap();
        let log = LogStore::open(JsonlFileBacking::new(dir.path().join("nope.jsonl"))).unwrap();
        assert!(log.is_empty());
    }
}
