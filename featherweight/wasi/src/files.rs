//! `MemFiles`: an in-memory byte-file store following the byte-stream
//! pattern (`docs/patterns/bytestream.md`).
//!
//! The reference target for the shim's fd layer, and a working example
//! of the pattern: files are byte values addressed by path, with ranged
//! reads at `{file}/at/{offset}/len/{n}`, `{file}/len`, appends at
//! `{file}/append`, positioned writes at `{file}/at/{offset}`, and the
//! store conventions (`Null` deletes, `Bytes` replaces). Reading a
//! directory prefix returns a map of child names to sizes.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// What a path names within the store.
enum Resolved {
    /// `{file}` itself.
    Whole(Vec<String>),
    /// `{file}/len`.
    Len(Vec<String>),
    /// `{file}/at/{offset}/len/{n}` (read) or `{file}/at/{offset}` (write).
    At {
        file: Vec<String>,
        offset: u64,
        len: Option<u64>,
    },
    /// `{file}/append`.
    Append(Vec<String>),
}

fn components(path: &Path) -> Vec<String> {
    path.iter().cloned().collect()
}

/// Parse the operation suffix off a file path.
fn resolve(path: &Path) -> Resolved {
    let parts = components(path);
    let n = parts.len();
    if n >= 5 && parts[n - 4] == "at" && parts[n - 2] == "len" {
        if let (Ok(offset), Ok(len)) = (parts[n - 3].parse(), parts[n - 1].parse()) {
            return Resolved::At {
                file: parts[..n - 4].to_vec(),
                offset,
                len: Some(len),
            };
        }
    }
    if n >= 3 && parts[n - 2] == "at" {
        if let Ok(offset) = parts[n - 1].parse() {
            return Resolved::At {
                file: parts[..n - 2].to_vec(),
                offset,
                len: None,
            };
        }
    }
    if n >= 2 && parts[n - 1] == "len" {
        return Resolved::Len(parts[..n - 1].to_vec());
    }
    if n >= 2 && parts[n - 1] == "append" {
        return Resolved::Append(parts[..n - 1].to_vec());
    }
    Resolved::Whole(parts)
}

/// An in-memory tree of byte files.
#[derive(Default)]
pub struct MemFiles {
    files: BTreeMap<Vec<String>, Vec<u8>>,
}

impl MemFiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Directly seed a file (tests, fixtures).
    pub fn insert(&mut self, path: &Path, bytes: Vec<u8>) {
        self.files.insert(components(path), bytes);
    }

    /// Direct contents access (tests).
    pub fn get(&self, path: &Path) -> Option<&Vec<u8>> {
        self.files.get(&components(path))
    }

    fn dir_listing(&self, prefix: &[String]) -> Option<Value> {
        let mut children = BTreeMap::new();
        for (file, bytes) in &self.files {
            if file.len() > prefix.len() && file[..prefix.len()] == *prefix {
                let name = file[prefix.len()].clone();
                let value = if file.len() == prefix.len() + 1 {
                    Value::Integer(bytes.len() as i64)
                } else {
                    Value::from("dir")
                };
                children.entry(name).or_insert(value);
            }
        }
        if children.is_empty() {
            None
        } else {
            Some(Value::Map(children))
        }
    }
}

impl Reader for MemFiles {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let value = match resolve(from) {
            Resolved::Whole(file) => match self.files.get(&file) {
                Some(bytes) => Some(Value::Bytes(bytes.clone())),
                None => self.dir_listing(&file),
            },
            Resolved::Len(file) => self
                .files
                .get(&file)
                .map(|bytes| Value::Integer(bytes.len() as i64)),
            Resolved::At {
                file,
                offset,
                len: Some(len),
            } => self.files.get(&file).map(|bytes| {
                // Stale offsets clamp (the pattern's rule).
                let start = (offset as usize).min(bytes.len());
                let end = start.saturating_add(len as usize).min(bytes.len());
                Value::Bytes(bytes[start..end].to_vec())
            }),
            // A ranged read needs a length; `at/{offset}` alone is a
            // write shape.
            Resolved::At { .. } | Resolved::Append(_) => None,
        };
        Ok(value.map(Record::parsed))
    }
}

impl Writer for MemFiles {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let value = data.into_value(&structfs_core_store::NoCodec)?;
        let bytes = match &value {
            Value::Bytes(bytes) => Some(bytes.clone()),
            Value::Null => None,
            // Strings are accepted as UTF-8 bytes for convenience.
            Value::String(text) => Some(text.clone().into_bytes()),
            _ => {
                return Err(Error::store(
                    "mem_files",
                    "write",
                    "files accept Bytes, String, or Null",
                ))
            }
        };
        match resolve(to) {
            Resolved::Whole(file) => {
                match bytes {
                    Some(bytes) => {
                        self.files.insert(file, bytes);
                    }
                    // Null deletes (the store convention; O_TRUNC's shape).
                    None => {
                        self.files.remove(&file);
                    }
                }
                Ok(to.clone())
            }
            Resolved::Append(file) => {
                let bytes = bytes
                    .ok_or_else(|| Error::store("mem_files", "append", "append requires bytes"))?;
                self.files
                    .entry(file)
                    .or_default()
                    .extend_from_slice(&bytes);
                Ok(to.clone())
            }
            Resolved::At {
                file,
                offset,
                len: None,
            } => {
                let bytes = bytes.ok_or_else(|| {
                    Error::store("mem_files", "write_at", "positioned write requires bytes")
                })?;
                let contents = self.files.entry(file).or_default();
                let offset = offset as usize;
                if contents.len() < offset {
                    contents.resize(offset, 0);
                }
                let end = offset + bytes.len();
                if contents.len() < end {
                    contents.resize(end, 0);
                }
                contents[offset..end].copy_from_slice(&bytes);
                Ok(to.clone())
            }
            _ => Err(Error::permission_denied(format!(
                "not a writable file path: {to}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    #[test]
    fn ranged_reads_and_appends() {
        let mut files = MemFiles::new();
        files
            .write(&path!("log/append"), Record::parsed(Value::from("hello ")))
            .unwrap();
        files
            .write(&path!("log/append"), Record::parsed(Value::from("world")))
            .unwrap();

        let len = files.read(&path!("log/len")).unwrap().unwrap();
        assert_eq!(len.as_value(), Some(&Value::Integer(11)));

        let chunk = files.read(&path!("log/at/6/len/5")).unwrap().unwrap();
        assert_eq!(chunk.as_value(), Some(&Value::Bytes(b"world".to_vec())));

        // Stale offsets clamp to empty.
        let past = files.read(&path!("log/at/99/len/5")).unwrap().unwrap();
        assert_eq!(past.as_value(), Some(&Value::Bytes(vec![])));
    }

    #[test]
    fn positioned_writes_extend_and_overwrite() {
        let mut files = MemFiles::new();
        files
            .write(
                &path!("f/at/0"),
                Record::parsed(Value::Bytes(b"abcdef".to_vec())),
            )
            .unwrap();
        files
            .write(
                &path!("f/at/2"),
                Record::parsed(Value::Bytes(b"XY".to_vec())),
            )
            .unwrap();
        assert_eq!(files.get(&path!("f")).unwrap(), b"abXYef");

        // Writing past the end zero-fills the gap.
        files
            .write(
                &path!("g/at/3"),
                Record::parsed(Value::Bytes(b"Z".to_vec())),
            )
            .unwrap();
        assert_eq!(files.get(&path!("g")).unwrap(), &vec![0, 0, 0, b'Z']);
    }

    #[test]
    fn null_deletes_and_bytes_replace() {
        let mut files = MemFiles::new();
        files
            .write(
                &path!("doomed"),
                Record::parsed(Value::Bytes(b"x".to_vec())),
            )
            .unwrap();
        files
            .write(&path!("doomed"), Record::parsed(Value::Null))
            .unwrap();
        assert!(files.read(&path!("doomed")).unwrap().is_none());
        assert!(files.read(&path!("doomed/len")).unwrap().is_none());
    }

    #[test]
    fn directory_listing() {
        let mut files = MemFiles::new();
        files.insert(&path!("docs/a"), b"1".to_vec());
        files.insert(&path!("docs/sub/b"), b"22".to_vec());

        let listing = files.read(&path!("docs")).unwrap().unwrap();
        match listing.as_value().unwrap() {
            Value::Map(map) => {
                assert_eq!(map.get("a"), Some(&Value::Integer(1)));
                assert_eq!(map.get("sub"), Some(&Value::from("dir")));
            }
            other => panic!("expected listing, got {other:?}"),
        }
    }
}
