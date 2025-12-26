//! Filesystem operations store.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read as IoRead, Seek, SeekFrom, Write as IoWrite};
use std::sync::atomic::{AtomicU64, Ordering};

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

static FS_HANDLE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// File open mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenMode {
    #[default]
    Read,
    Write,
    Append,
    ReadWrite,
    CreateNew,
}

/// An open file handle.
struct FileHandle {
    file: File,
    path: String,
    #[allow(dead_code)]
    mode: OpenMode,
}

/// Store for filesystem operations.
pub struct FsStore {
    handles: HashMap<u64, FileHandle>,
}

impl FsStore {
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    fn next_handle_id() -> u64 {
        FS_HANDLE_COUNTER.fetch_add(1, Ordering::SeqCst)
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let mut map = BTreeMap::new();
            map.insert(
                "open".to_string(),
                Value::String("Write {path, mode} to get handle".to_string()),
            );
            map.insert(
                "handles".to_string(),
                Value::String("Open file handles".to_string()),
            );
            map.insert(
                "stat".to_string(),
                Value::String("Write {path} to get file info".to_string()),
            );
            map.insert(
                "mkdir".to_string(),
                Value::String("Write {path} to create directory".to_string()),
            );
            map.insert(
                "rmdir".to_string(),
                Value::String("Write {path} to remove directory".to_string()),
            );
            map.insert(
                "unlink".to_string(),
                Value::String("Write {path} to delete file".to_string()),
            );
            map.insert(
                "rename".to_string(),
                Value::String("Write {from, to} to rename".to_string()),
            );
            return Ok(Some(Value::Map(map)));
        }

        if path[0] == "handles" {
            return self.read_handles(path);
        }

        Ok(None)
    }

    fn read_handles(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.len() == 1 {
            let handles: Vec<Value> = self
                .handles
                .iter()
                .map(|(id, h)| {
                    let mut m = BTreeMap::new();
                    m.insert("id".to_string(), Value::Integer(*id as i64));
                    m.insert("path".to_string(), Value::String(h.path.clone()));
                    Value::Map(m)
                })
                .collect();
            return Ok(Some(Value::Array(handles)));
        }

        let handle_id: u64 = path[1]
            .parse()
            .map_err(|_| Error::store("fs", "read", format!("Invalid handle ID: {}", path[1])))?;

        let handle = self
            .handles
            .get(&handle_id)
            .ok_or_else(|| Error::store("fs", "read", format!("Handle {} not found", handle_id)))?;

        if path.len() == 2 {
            let mut m = BTreeMap::new();
            m.insert("handle".to_string(), Value::Integer(handle_id as i64));
            m.insert("path".to_string(), Value::String(handle.path.clone()));
            return Ok(Some(Value::Map(m)));
        }

        if path.len() == 3 && path[2] == "meta" {
            let metadata = fs::metadata(&handle.path)?;

            let mut m = BTreeMap::new();
            m.insert("size".to_string(), Value::Integer(metadata.len() as i64));
            m.insert("is_file".to_string(), Value::Bool(metadata.is_file()));
            m.insert("is_dir".to_string(), Value::Bool(metadata.is_dir()));
            return Ok(Some(Value::Map(m)));
        }

        Ok(None)
    }

    fn parse_open_mode(value: &Value) -> OpenMode {
        if let Value::Map(map) = value {
            if let Some(Value::String(mode)) = map.get("mode") {
                return match mode.as_str() {
                    "write" => OpenMode::Write,
                    "append" => OpenMode::Append,
                    "readwrite" => OpenMode::ReadWrite,
                    "createnew" => OpenMode::CreateNew,
                    _ => OpenMode::Read,
                };
            }
        }
        OpenMode::Read
    }

    fn get_path_from_value(value: &Value) -> Option<String> {
        if let Value::Map(map) = value {
            if let Some(Value::String(p)) = map.get("path") {
                return Some(p.clone());
            }
        }
        None
    }

    fn write_handle(&mut self, path: &Path, value: &Value) -> Result<Path, Error> {
        if path.len() < 2 {
            return Err(Error::store("fs", "write", "Invalid handle path"));
        }

        let handle_id: u64 = path[1]
            .parse()
            .map_err(|_| Error::store("fs", "write", format!("Invalid handle ID: {}", path[1])))?;

        // Handle close operation
        if path.len() == 3 && path[2] == "close" {
            self.handles.remove(&handle_id).ok_or_else(|| {
                Error::store("fs", "write", format!("Handle {} not found", handle_id))
            })?;
            return Ok(path.clone());
        }

        // Handle seek operation
        if path.len() == 3 && path[2] == "seek" {
            let handle = self.handles.get_mut(&handle_id).ok_or_else(|| {
                Error::store("fs", "write", format!("Handle {} not found", handle_id))
            })?;

            let pos = if let Value::Map(map) = value {
                if let Some(Value::Integer(p)) = map.get("pos") {
                    *p as u64
                } else {
                    return Err(Error::store("fs", "seek", "seek requires 'pos' field"));
                }
            } else {
                return Err(Error::store("fs", "seek", "seek requires a map with 'pos'"));
            };

            handle.file.seek(SeekFrom::Start(pos))?;

            return Ok(path.clone());
        }

        // Write content to file
        if path.len() == 2 {
            let handle = self.handles.get_mut(&handle_id).ok_or_else(|| {
                Error::store("fs", "write", format!("Handle {} not found", handle_id))
            })?;

            let content = match value {
                Value::String(s) => {
                    // Try to decode as base64, fall back to UTF-8
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                        .unwrap_or_else(|_| s.as_bytes().to_vec())
                }
                Value::Bytes(b) => b.to_vec(),
                _ => {
                    return Err(Error::store(
                        "fs",
                        "write",
                        "File content must be a string or bytes",
                    ));
                }
            };

            handle.file.write_all(&content)?;

            return Ok(path.clone());
        }

        Err(Error::store(
            "fs",
            "write",
            format!("Unknown handle operation: {}", path),
        ))
    }
}

impl Default for FsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for FsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Handle reading file content from handles
        if from.len() == 2 && from[0] == "handles" {
            let handle_id: u64 = from[1].parse().map_err(|_| {
                Error::store("fs", "read", format!("Invalid handle ID: {}", from[1]))
            })?;

            let handle = self.handles.get_mut(&handle_id).ok_or_else(|| {
                Error::store("fs", "read", format!("Handle {} not found", handle_id))
            })?;

            let mut buffer = Vec::new();
            handle.file.read_to_end(&mut buffer)?;

            // Return as base64-encoded bytes
            let encoded =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buffer);
            return Ok(Some(Record::parsed(Value::String(encoded))));
        }

        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for FsStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::store("fs", "write", "Cannot write to fs root"));
        }

        let value = data.into_value(&NoCodec)?;

        // Handle writes to handles
        if to[0] == "handles" {
            return self.write_handle(to, &value);
        }

        if to.len() != 1 {
            return Err(Error::store(
                "fs",
                "write",
                format!("Invalid fs path: {}", to),
            ));
        }

        match to[0].as_str() {
            "open" => {
                let file_path = Self::get_path_from_value(&value)
                    .ok_or_else(|| Error::store("fs", "open", "open requires 'path' field"))?;

                let mode = Self::parse_open_mode(&value);

                let file = match mode {
                    OpenMode::Read => File::open(&file_path),
                    OpenMode::Write => File::create(&file_path),
                    OpenMode::Append => OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(&file_path),
                    OpenMode::ReadWrite => OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .truncate(false)
                        .open(&file_path),
                    OpenMode::CreateNew => OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&file_path),
                }?;

                let handle_id = Self::next_handle_id();
                self.handles.insert(
                    handle_id,
                    FileHandle {
                        file,
                        path: file_path,
                        mode,
                    },
                );

                Ok(Path::parse(&format!("handles/{}", handle_id)).unwrap())
            }
            "stat" => {
                let file_path = Self::get_path_from_value(&value)
                    .ok_or_else(|| Error::store("fs", "stat", "stat requires 'path' field"))?;

                let _metadata = fs::metadata(&file_path)?;

                Ok(to.clone())
            }
            "mkdir" => {
                let file_path = Self::get_path_from_value(&value)
                    .ok_or_else(|| Error::store("fs", "mkdir", "mkdir requires 'path' field"))?;

                let recursive = if let Value::Map(map) = &value {
                    matches!(map.get("recursive"), Some(Value::Bool(true)))
                } else {
                    false
                };

                if recursive {
                    fs::create_dir_all(&file_path)?;
                } else {
                    fs::create_dir(&file_path)?;
                }

                Ok(to.clone())
            }
            "rmdir" => {
                let file_path = Self::get_path_from_value(&value)
                    .ok_or_else(|| Error::store("fs", "rmdir", "rmdir requires 'path' field"))?;

                fs::remove_dir(&file_path)?;

                Ok(to.clone())
            }
            "unlink" => {
                let file_path = Self::get_path_from_value(&value)
                    .ok_or_else(|| Error::store("fs", "unlink", "unlink requires 'path' field"))?;

                fs::remove_file(&file_path)?;

                Ok(to.clone())
            }
            "rename" => {
                let (from_path, to_path) = if let Value::Map(map) = &value {
                    let from = map
                        .get("from")
                        .and_then(|v| {
                            if let Value::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| {
                            Error::store("fs", "rename", "rename requires 'from' field")
                        })?;
                    let to_str = map
                        .get("to")
                        .and_then(|v| {
                            if let Value::String(s) = v {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| {
                            Error::store("fs", "rename", "rename requires 'to' field")
                        })?;
                    (from, to_str)
                } else {
                    return Err(Error::store(
                        "fs",
                        "rename",
                        "rename requires a map with 'from' and 'to'",
                    ));
                };

                fs::rename(&from_path, &to_path)?;

                Ok(to.clone())
            }
            _ => Err(Error::store(
                "fs",
                "write",
                format!("Unknown fs operation: {}", to[0]),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use structfs_core_store::path;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn open_and_read_file() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "Hello, world!").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open the file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read from the handle
        let record = store.read(&handle_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                let decoded =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &s).unwrap();
                let content = String::from_utf8(decoded).unwrap();
                assert!(content.contains("Hello"));
            }
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn list_handles() {
        let mut store = FsStore::new();
        let record = store.read(&path!("handles")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Array(_) => {}
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn read_root() {
        let mut store = FsStore::new();
        let record = store.read(&path!("")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("open"));
                assert!(map.contains_key("handles"));
                assert!(map.contains_key("stat"));
                assert!(map.contains_key("mkdir"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_nonexistent() {
        let mut store = FsStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn open_write_close() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open for write
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str.clone()));
        open_map.insert("mode".to_string(), Value::String("write".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Write content (as UTF-8 string)
        store
            .write(
                &handle_path,
                Record::parsed(Value::String("test content".to_string())),
            )
            .unwrap();

        // Close handle
        let close_path = handle_path.join(&path!("close"));
        store
            .write(&close_path, Record::parsed(Value::Null))
            .unwrap();

        // Verify file content
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "test content");
    }

    #[test]
    fn open_write_bytes() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test_bytes.bin");
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open for write
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str.clone()));
        open_map.insert("mode".to_string(), Value::String("write".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Write content as bytes
        store
            .write(&handle_path, Record::parsed(Value::Bytes(vec![1, 2, 3, 4])))
            .unwrap();

        // Verify file content
        let content = std::fs::read(&file_path).unwrap();
        assert_eq!(content, vec![1, 2, 3, 4]);
    }

    #[test]
    fn open_append_mode() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("append.txt");
        std::fs::write(&file_path, "first").unwrap();
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str.clone()));
        open_map.insert("mode".to_string(), Value::String("append".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        store
            .write(
                &handle_path,
                Record::parsed(Value::String("second".to_string())),
            )
            .unwrap();

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "firstsecond");
    }

    #[test]
    fn open_readwrite_mode() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("readwrite.txt");
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str));
        open_map.insert("mode".to_string(), Value::String("readwrite".to_string()));

        let result = store.write(&path!("open"), Record::parsed(Value::Map(open_map)));
        assert!(result.is_ok());
    }

    #[test]
    fn open_createnew_mode() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("new.txt");
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str));
        open_map.insert("mode".to_string(), Value::String("createnew".to_string()));

        let result = store.write(&path!("open"), Record::parsed(Value::Map(open_map)));
        assert!(result.is_ok());
    }

    #[test]
    fn open_missing_path_error() {
        let mut store = FsStore::new();

        let open_map = BTreeMap::new();
        let result = store.write(&path!("open"), Record::parsed(Value::Map(open_map)));
        assert!(result.is_err());
    }

    #[test]
    fn open_nonexistent_file_error() {
        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert(
            "path".to_string(),
            Value::String("/nonexistent/path/12345".to_string()),
        );
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let result = store.write(&path!("open"), Record::parsed(Value::Map(open_map)));
        assert!(result.is_err());
    }

    #[test]
    fn handle_meta() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read meta
        let meta_path = handle_path.join(&path!("meta"));
        let record = store.read(&meta_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("size"));
                assert!(map.contains_key("is_file"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn handle_seek() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "0123456789").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Seek to position
        let seek_path = handle_path.join(&path!("seek"));
        let mut seek_map = BTreeMap::new();
        seek_map.insert("pos".to_string(), Value::Integer(5));

        store
            .write(&seek_path, Record::parsed(Value::Map(seek_map)))
            .unwrap();
    }

    #[test]
    fn handle_seek_missing_pos_error() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let seek_path = handle_path.join(&path!("seek"));
        let seek_map = BTreeMap::new();

        let result = store.write(&seek_path, Record::parsed(Value::Map(seek_map)));
        assert!(result.is_err());
    }

    #[test]
    fn handle_seek_invalid_type_error() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let seek_path = handle_path.join(&path!("seek"));
        let result = store.write(&seek_path, Record::parsed(Value::String("5".to_string())));
        assert!(result.is_err());
    }

    #[test]
    fn handle_invalid_id_error() {
        let mut store = FsStore::new();
        let result = store.read(&path!("handles/invalid"));
        assert!(result.is_err());
    }

    #[test]
    fn handle_not_found_error() {
        let mut store = FsStore::new();
        let result = store.read(&path!("handles/999999"));
        assert!(result.is_err());
    }

    #[test]
    fn mkdir_and_rmdir() {
        let temp_dir = TempDir::new().unwrap();
        let new_dir = temp_dir.path().join("newdir");
        let path_str = new_dir.to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Create directory
        let mut mkdir_map = BTreeMap::new();
        mkdir_map.insert("path".to_string(), Value::String(path_str.clone()));

        store
            .write(&path!("mkdir"), Record::parsed(Value::Map(mkdir_map)))
            .unwrap();

        assert!(new_dir.exists());

        // Remove directory
        let mut rmdir_map = BTreeMap::new();
        rmdir_map.insert("path".to_string(), Value::String(path_str));

        store
            .write(&path!("rmdir"), Record::parsed(Value::Map(rmdir_map)))
            .unwrap();

        assert!(!new_dir.exists());
    }

    #[test]
    fn mkdir_recursive() {
        let temp_dir = TempDir::new().unwrap();
        let deep_dir = temp_dir.path().join("a/b/c");
        let path_str = deep_dir.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut mkdir_map = BTreeMap::new();
        mkdir_map.insert("path".to_string(), Value::String(path_str));
        mkdir_map.insert("recursive".to_string(), Value::Bool(true));

        store
            .write(&path!("mkdir"), Record::parsed(Value::Map(mkdir_map)))
            .unwrap();

        assert!(deep_dir.exists());
    }

    #[test]
    fn stat_file() {
        let temp = NamedTempFile::new().unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut stat_map = BTreeMap::new();
        stat_map.insert("path".to_string(), Value::String(temp_path));

        store
            .write(&path!("stat"), Record::parsed(Value::Map(stat_map)))
            .unwrap();
    }

    #[test]
    fn unlink_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("to_delete.txt");
        std::fs::write(&file_path, "content").unwrap();
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut unlink_map = BTreeMap::new();
        unlink_map.insert("path".to_string(), Value::String(path_str));

        store
            .write(&path!("unlink"), Record::parsed(Value::Map(unlink_map)))
            .unwrap();

        assert!(!file_path.exists());
    }

    #[test]
    fn rename_file() {
        let temp_dir = TempDir::new().unwrap();
        let old_path = temp_dir.path().join("old.txt");
        let new_path = temp_dir.path().join("new.txt");
        std::fs::write(&old_path, "content").unwrap();

        let mut store = FsStore::new();

        let mut rename_map = BTreeMap::new();
        rename_map.insert(
            "from".to_string(),
            Value::String(old_path.to_string_lossy().to_string()),
        );
        rename_map.insert(
            "to".to_string(),
            Value::String(new_path.to_string_lossy().to_string()),
        );

        store
            .write(&path!("rename"), Record::parsed(Value::Map(rename_map)))
            .unwrap();

        assert!(!old_path.exists());
        assert!(new_path.exists());
    }

    #[test]
    fn rename_missing_from_error() {
        let mut store = FsStore::new();

        let mut rename_map = BTreeMap::new();
        rename_map.insert("to".to_string(), Value::String("/tmp/new".to_string()));

        let result = store.write(&path!("rename"), Record::parsed(Value::Map(rename_map)));
        assert!(result.is_err());
    }

    #[test]
    fn rename_missing_to_error() {
        let mut store = FsStore::new();

        let mut rename_map = BTreeMap::new();
        rename_map.insert("from".to_string(), Value::String("/tmp/old".to_string()));

        let result = store.write(&path!("rename"), Record::parsed(Value::Map(rename_map)));
        assert!(result.is_err());
    }

    #[test]
    fn rename_invalid_type_error() {
        let mut store = FsStore::new();
        let result = store.write(
            &path!("rename"),
            Record::parsed(Value::String("x".to_string())),
        );
        assert!(result.is_err());
    }

    #[test]
    fn write_to_root_error() {
        let mut store = FsStore::new();
        let result = store.write(&path!(""), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn write_unknown_operation_error() {
        let mut store = FsStore::new();
        let result = store.write(&path!("unknown"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn write_invalid_content_type_error() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str));
        open_map.insert("mode".to_string(), Value::String("write".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Try to write an invalid type
        let result = store.write(&handle_path, Record::parsed(Value::Integer(123)));
        assert!(result.is_err());
    }

    #[test]
    fn close_nonexistent_handle_error() {
        let mut store = FsStore::new();
        let result = store.write(&path!("handles/999999/close"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn default_impl() {
        let store = FsStore::default();
        assert!(std::ptr::eq(&store as *const _, &store as *const _));
    }

    #[test]
    fn open_mode_default() {
        let mode = OpenMode::default();
        assert_eq!(mode, OpenMode::Read);
    }
}
