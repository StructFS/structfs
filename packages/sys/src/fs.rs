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

        let handle_id: u64 = path[1].parse().map_err(|_| Error::Other {
            message: format!("Invalid handle ID: {}", path[1]),
        })?;

        let handle = self.handles.get(&handle_id).ok_or_else(|| Error::Other {
            message: format!("Handle {} not found", handle_id),
        })?;

        if path.len() == 2 {
            let mut m = BTreeMap::new();
            m.insert("handle".to_string(), Value::Integer(handle_id as i64));
            m.insert("path".to_string(), Value::String(handle.path.clone()));
            return Ok(Some(Value::Map(m)));
        }

        if path.len() == 3 && path[2] == "meta" {
            let metadata = fs::metadata(&handle.path).map_err(|e| Error::Other {
                message: format!("Failed to get metadata: {}", e),
            })?;

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
            return Err(Error::Other {
                message: "Invalid handle path".to_string(),
            });
        }

        let handle_id: u64 = path[1].parse().map_err(|_| Error::Other {
            message: format!("Invalid handle ID: {}", path[1]),
        })?;

        // Handle close operation
        if path.len() == 3 && path[2] == "close" {
            self.handles
                .remove(&handle_id)
                .ok_or_else(|| Error::Other {
                    message: format!("Handle {} not found", handle_id),
                })?;
            return Ok(path.clone());
        }

        // Handle seek operation
        if path.len() == 3 && path[2] == "seek" {
            let handle = self
                .handles
                .get_mut(&handle_id)
                .ok_or_else(|| Error::Other {
                    message: format!("Handle {} not found", handle_id),
                })?;

            let pos = if let Value::Map(map) = value {
                if let Some(Value::Integer(p)) = map.get("pos") {
                    *p as u64
                } else {
                    return Err(Error::Other {
                        message: "seek requires 'pos' field".to_string(),
                    });
                }
            } else {
                return Err(Error::Other {
                    message: "seek requires a map with 'pos'".to_string(),
                });
            };

            handle
                .file
                .seek(SeekFrom::Start(pos))
                .map_err(|e| Error::Other {
                    message: format!("Seek failed: {}", e),
                })?;

            return Ok(path.clone());
        }

        // Write content to file
        if path.len() == 2 {
            let handle = self
                .handles
                .get_mut(&handle_id)
                .ok_or_else(|| Error::Other {
                    message: format!("Handle {} not found", handle_id),
                })?;

            let content = match value {
                Value::String(s) => {
                    // Try to decode as base64, fall back to UTF-8
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                        .unwrap_or_else(|_| s.as_bytes().to_vec())
                }
                Value::Bytes(b) => b.to_vec(),
                _ => {
                    return Err(Error::Other {
                        message: "File content must be a string or bytes".to_string(),
                    });
                }
            };

            handle.file.write_all(&content).map_err(|e| Error::Other {
                message: format!("Failed to write to file: {}", e),
            })?;

            return Ok(path.clone());
        }

        Err(Error::Other {
            message: format!("Unknown handle operation: {}", path),
        })
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
            let handle_id: u64 = from[1].parse().map_err(|_| Error::Other {
                message: format!("Invalid handle ID: {}", from[1]),
            })?;

            let handle = self
                .handles
                .get_mut(&handle_id)
                .ok_or_else(|| Error::Other {
                    message: format!("Handle {} not found", handle_id),
                })?;

            let mut buffer = Vec::new();
            handle
                .file
                .read_to_end(&mut buffer)
                .map_err(|e| Error::Other {
                    message: format!("Failed to read file: {}", e),
                })?;

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
            return Err(Error::Other {
                message: "Cannot write to fs root".to_string(),
            });
        }

        let value = data.into_value(&NoCodec)?;

        // Handle writes to handles
        if to[0] == "handles" {
            return self.write_handle(to, &value);
        }

        if to.len() != 1 {
            return Err(Error::Other {
                message: format!("Invalid fs path: {}", to),
            });
        }

        match to[0].as_str() {
            "open" => {
                let file_path = Self::get_path_from_value(&value).ok_or_else(|| Error::Other {
                    message: "open requires 'path' field".to_string(),
                })?;

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
                }
                .map_err(|e| Error::Other {
                    message: format!("Failed to open '{}': {}", file_path, e),
                })?;

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
                let file_path = Self::get_path_from_value(&value).ok_or_else(|| Error::Other {
                    message: "stat requires 'path' field".to_string(),
                })?;

                let _metadata = fs::metadata(&file_path).map_err(|e| Error::Other {
                    message: format!("stat failed: {}", e),
                })?;

                Ok(to.clone())
            }
            "mkdir" => {
                let file_path = Self::get_path_from_value(&value).ok_or_else(|| Error::Other {
                    message: "mkdir requires 'path' field".to_string(),
                })?;

                let recursive = if let Value::Map(map) = &value {
                    matches!(map.get("recursive"), Some(Value::Bool(true)))
                } else {
                    false
                };

                if recursive {
                    fs::create_dir_all(&file_path)
                } else {
                    fs::create_dir(&file_path)
                }
                .map_err(|e| Error::Other {
                    message: format!("mkdir failed: {}", e),
                })?;

                Ok(to.clone())
            }
            "rmdir" => {
                let file_path = Self::get_path_from_value(&value).ok_or_else(|| Error::Other {
                    message: "rmdir requires 'path' field".to_string(),
                })?;

                fs::remove_dir(&file_path).map_err(|e| Error::Other {
                    message: format!("rmdir failed: {}", e),
                })?;

                Ok(to.clone())
            }
            "unlink" => {
                let file_path = Self::get_path_from_value(&value).ok_or_else(|| Error::Other {
                    message: "unlink requires 'path' field".to_string(),
                })?;

                fs::remove_file(&file_path).map_err(|e| Error::Other {
                    message: format!("unlink failed: {}", e),
                })?;

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
                        .ok_or_else(|| Error::Other {
                            message: "rename requires 'from' field".to_string(),
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
                        .ok_or_else(|| Error::Other {
                            message: "rename requires 'to' field".to_string(),
                        })?;
                    (from, to_str)
                } else {
                    return Err(Error::Other {
                        message: "rename requires a map with 'from' and 'to'".to_string(),
                    });
                };

                fs::rename(&from_path, &to_path).map_err(|e| Error::Other {
                    message: format!("rename failed: {}", e),
                })?;

                Ok(to.clone())
            }
            _ => Err(Error::Other {
                message: format!("Unknown fs operation: {}", to[0]),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use structfs_core_store::path;
    use tempfile::NamedTempFile;

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
}
