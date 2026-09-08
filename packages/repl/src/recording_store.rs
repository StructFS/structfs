//! A recording directory as one read-only tree.
//!
//! `fw run --record DIR` leaves a session log (`session.jsonl`) and one
//! transcript per block (`<scope>/<block>.transcript.jsonl`). Mounting
//! that directory (`write /ctx/mounts/rec {"type": "recording", "path":
//! "DIR"}`) serves the whole recording as a browsable tree:
//!
//! ```text
//! read /rec                          # what the recording holds
//! read /rec/session/len              # the timeline's length
//! read /rec/session/entries/from/0   # page the timeline
//! read /rec/fw-demo/shell/entries/3  # one transcript entry
//! ```
//!
//! Every log speaks the append-log convention (`len`, `entries/{n}`,
//! `entries/from/{n}` with the `{items, next, status}` tail envelope),
//! so paging works the same way everywhere. Writes are denied: a
//! recording is evidence, and evidence stays evidence.

use collection_literals::btree;
use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};
use structfs_json_store::{JsonlFileBacking, LogStore};

/// One recording directory, each JSONL log mounted at its key path.
pub struct RecordingStore {
    /// Longest prefix first, so routing finds the most specific log.
    logs: Vec<(Path, LogStore<JsonlFileBacking>)>,
}

impl RecordingStore {
    /// Open every `*.jsonl` under `dir`: `session.jsonl` serves at
    /// `session`, `<scope>/<block>.transcript.jsonl` at
    /// `<scope>/<block>`, and any other `x.jsonl` at `x`.
    pub fn open(dir: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let mut files = Vec::new();
        collect_jsonl(dir, dir, &mut files)?;
        if files.is_empty() {
            return Err(Error::store(
                "recording",
                "open",
                format!("no .jsonl logs under {}", dir.display()),
            ));
        }
        let mut logs = Vec::with_capacity(files.len());
        for (at, file) in files {
            logs.push((at, LogStore::open(JsonlFileBacking::new(file))?));
        }
        logs.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        Ok(Self { logs })
    }

    /// The children directly under `from`, per the map convention —
    /// `None` when nothing in the recording lives below it.
    fn listing(&self, from: &Path) -> Option<Value> {
        let mut children: BTreeMap<String, Value> = BTreeMap::new();
        for (prefix, log) in &self.logs {
            let Some(rel) = prefix.strip_prefix(from) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            let name = rel[0].clone();
            let label = if rel.len() == 1 {
                format!("append log ({} entries)", log.len())
            } else {
                "recording subtree".to_string()
            };
            children.insert(name, Value::String(label));
        }
        (!children.is_empty()).then_some(Value::Map(children))
    }

    fn docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Recording".into()),
            "description".into() => Value::String(
                "A recorded run, read-only: the session timeline at 'session', \
                 each block's transcript at its assembly-scoped key. Page any \
                 log with entries/from/{n}; read len for its length."
                    .into(),
            ),
        })
    }
}

fn collect_jsonl(
    root: &std::path::Path,
    dir: &std::path::Path,
    into: &mut Vec<(Path, std::path::PathBuf)>,
) -> Result<(), Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(root, &path, into)?;
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name
            .strip_suffix(".transcript.jsonl")
            .or_else(|| name.strip_suffix(".jsonl"))
        else {
            continue;
        };
        let mut mount = path
            .parent()
            .and_then(|p| p.strip_prefix(root).ok())
            .map(|p| p.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/"))
            .unwrap_or_default();
        if mount.is_empty() {
            mount = stem.to_string();
        } else {
            mount = format!("{mount}/{stem}");
        }
        let at = Path::parse(&mount).map_err(|e| {
            Error::store(
                "recording",
                "open",
                format!("{} does not name a mountable log: {e}", path.display()),
            )
        })?;
        into.push((at, path));
    }
    Ok(())
}

impl Reader for RecordingStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        if from.len() == 1 && from[0] == "docs" {
            return Ok(Some(Record::parsed(Self::docs())));
        }
        for (prefix, log) in &mut self.logs {
            if let Some(rel) = from.strip_prefix(prefix) {
                return log.read(&rel);
            }
        }
        Ok(self.listing(from).map(Record::parsed))
    }
}

impl Writer for RecordingStore {
    fn write(&mut self, to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::permission_denied(format!(
            "a recording is read-only evidence; nothing at '{to}' takes writes"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    fn recording_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.jsonl"),
            "{\"seq\":0,\"block\":\"demo/shell\",\"op\":\"read\",\"path\":\"iso/random/uuid\",\"outcome\":\"found\",\"entry\":0}\n\
             {\"seq\":1,\"block\":\"demo/kv\",\"op\":\"write\",\"path\":\"greeting\",\"outcome\":\"wrote\",\"entry\":0}\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("demo")).unwrap();
        std::fs::write(
            dir.path().join("demo/shell.transcript.jsonl"),
            "{\"op\":\"read\",\"path\":\"iso/random/uuid\",\"answer\":{\"found\":{\"parsed\":\"u-1\"}}}\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn a_recording_serves_session_and_transcripts_as_one_tree() {
        let dir = recording_dir();
        let mut store = RecordingStore::open(dir.path()).unwrap();

        // The root lists what the recording holds.
        let root = store.read(&path!("")).unwrap().unwrap();
        let Some(Value::Map(map)) = root.as_value() else {
            panic!("expected a listing");
        };
        assert!(map.contains_key("session"), "{map:?}");
        assert!(map.contains_key("demo"), "{map:?}");

        // The session timeline pages with the cursor convention.
        let len = store.read(&path!("session/len")).unwrap().unwrap();
        assert_eq!(len.as_value(), Some(&Value::Integer(2)));
        let page = store
            .read(&path!("session/entries/from/1"))
            .unwrap()
            .unwrap();
        let Some(Value::Map(envelope)) = page.as_value() else {
            panic!("expected a tail envelope");
        };
        assert!(matches!(
            envelope.get("items"),
            Some(Value::Array(items)) if items.len() == 1
        ));

        // A block transcript serves at its assembly-scoped key.
        let entry = store.read(&path!("demo/shell/entries/0")).unwrap().unwrap();
        let Some(Value::Map(entry)) = entry.as_value() else {
            panic!("expected a transcript entry");
        };
        assert_eq!(entry.get("path"), Some(&Value::from("iso/random/uuid")));

        // Nothing below an unknown path.
        assert!(store.read(&path!("nowhere")).unwrap().is_none());
    }

    #[test]
    fn a_recording_is_read_only_evidence() {
        let dir = recording_dir();
        let mut store = RecordingStore::open(dir.path()).unwrap();
        let denied = store
            .write(&path!("session/append"), Record::parsed(Value::Null))
            .unwrap_err();
        assert!(matches!(denied, Error::PermissionDenied { .. }));
    }

    #[test]
    fn an_empty_directory_is_refused_loudly() {
        let dir = tempfile::tempdir().unwrap();
        let Err(missing) = RecordingStore::open(dir.path()).map(|_| ()) else {
            panic!("an empty directory must be refused");
        };
        assert!(missing.to_string().contains("no .jsonl logs"), "{missing}");
    }
}
