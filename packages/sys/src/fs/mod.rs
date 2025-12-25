//! Filesystem operations store.
//!
//! Provides filesystem operations through StructFS paths.
//!
//! ## Paths
//!
//! ### File Operations
//! - `fs/open` - Write `{"path": "/file", "mode": "read"}` → returns handle path
//! - `fs/handles/{id}` - Read/write file content through handle
//! - `fs/handles/{id}/meta` - Read file metadata
//! - `fs/handles/{id}/seek` - Write `{"pos": N}` to seek
//! - `fs/handles/{id}/close` - Write to close handle
//!
//! ### Directory Operations
//! - `fs/stat` - Write `{"path": "/some/path"}` to get file info
//! - `fs/readdir` - Write `{"path": "/some/dir"}` to list directory
//! - `fs/mkdir` - Write `{"path": "/some/dir"}` to create directory
//! - `fs/rmdir` - Write `{"path": "/some/dir"}` to remove directory
//! - `fs/unlink` - Write `{"path": "/some/file"}` to delete file
//! - `fs/rename` - Write `{"from": "/a", "to": "/b"}` to rename

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write as IoWrite};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};
use structfs_store::{Error, Path, Reader, Writer};

/// Supported encodings for file content
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    /// Base64 encoding (default, safe for binary)
    #[default]
    Base64,
    /// UTF-8 text
    Utf8,
    /// Latin-1 / ISO-8859-1
    Latin1,
    /// ASCII (7-bit)
    Ascii,
}

/// File open mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OpenMode {
    /// Read-only
    #[default]
    Read,
    /// Write-only (creates or truncates)
    Write,
    /// Append (creates if needed)
    Append,
    /// Read and write
    ReadWrite,
    /// Create new file (fails if exists)
    CreateNew,
}

/// An open file handle
struct FileHandle {
    file: File,
    path: String,
    mode: OpenMode,
    encoding: Encoding,
}

static HANDLE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        HANDLE_COUNTER.fetch_add(1, Ordering::SeqCst)
    }

    fn read_value(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.is_empty() {
            return Ok(Some(json!({
                "open": "Write {\"path\": \"...\", \"mode\": \"read|write|append|readwrite|createnew\", \"encoding\": \"base64|utf8|latin1|ascii\"} - returns handle",
                "handles": "Open file handles",
                "stat": "Write {\"path\": \"...\"} to get file info",
                "readdir": "Write {\"path\": \"...\"} to list directory",
                "mkdir": "Write {\"path\": \"...\", \"recursive\": bool} to create directory",
                "rmdir": "Write {\"path\": \"...\"} to remove directory",
                "unlink": "Write {\"path\": \"...\"} to delete file",
                "rename": "Write {\"from\": \"...\", \"to\": \"...\"} to rename"
            })));
        }

        // Check for handles path
        if path.components[0] == "handles" {
            return self.read_handle(path);
        }

        Ok(None)
    }

    fn read_handle(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.components.len() == 1 {
            // List all handles
            let handles: Vec<JsonValue> = self
                .handles
                .iter()
                .map(|(id, h)| {
                    json!({
                        "id": id,
                        "path": h.path,
                        "mode": format!("{:?}", h.mode).to_lowercase(),
                        "encoding": format!("{:?}", h.encoding).to_lowercase(),
                    })
                })
                .collect();
            return Ok(Some(json!(handles)));
        }

        // Parse handle ID
        let handle_id: u64 = path.components[1]
            .parse()
            .map_err(|_| Error::ImplementationFailure {
                message: format!("Invalid handle ID: {}", path.components[1]),
            })?;

        let handle = self
            .handles
            .get(&handle_id)
            .ok_or_else(|| Error::ImplementationFailure {
                message: format!("Handle {} not found", handle_id),
            })?;

        if path.components.len() == 2 {
            // Read file content
            // We need mutable access, so this is handled specially
            return Ok(Some(json!({
                "note": "Use read with mutable access to get file content",
                "handle": handle_id,
                "path": handle.path,
                "encoding": format!("{:?}", handle.encoding).to_lowercase(),
            })));
        }

        // Handle sub-paths
        match path.components[2].as_str() {
            "meta" => {
                let metadata = fs::metadata(&handle.path).map_err(|e| {
                    Error::ImplementationFailure {
                        message: format!("Failed to get metadata: {}", e),
                    }
                })?;

                #[cfg(unix)]
                let result = json!({
                    "size": metadata.len(),
                    "mode": metadata.mode(),
                    "uid": metadata.uid(),
                    "gid": metadata.gid(),
                });

                #[cfg(not(unix))]
                let result = json!({
                    "size": metadata.len(),
                });

                Ok(Some(result))
            }
            _ => Ok(None),
        }
    }

    fn encode_content(&self, bytes: &[u8], encoding: Encoding) -> Result<JsonValue, Error> {
        match encoding {
            Encoding::Base64 => Ok(JsonValue::String(STANDARD.encode(bytes))),
            Encoding::Utf8 => {
                let s = std::str::from_utf8(bytes).map_err(|e| Error::ImplementationFailure {
                    message: format!("Invalid UTF-8: {}", e),
                })?;
                Ok(JsonValue::String(s.to_string()))
            }
            Encoding::Latin1 => {
                // Latin1 is a direct byte-to-char mapping for 0-255
                let s: String = bytes.iter().map(|&b| b as char).collect();
                Ok(JsonValue::String(s))
            }
            Encoding::Ascii => {
                for &b in bytes {
                    if b > 127 {
                        return Err(Error::ImplementationFailure {
                            message: format!("Non-ASCII byte: {}", b),
                        });
                    }
                }
                let s = std::str::from_utf8(bytes).map_err(|e| Error::ImplementationFailure {
                    message: format!("Invalid ASCII: {}", e),
                })?;
                Ok(JsonValue::String(s.to_string()))
            }
        }
    }

    fn decode_content(&self, value: &JsonValue, encoding: Encoding) -> Result<Vec<u8>, Error> {
        let s = value.as_str().ok_or_else(|| Error::ImplementationFailure {
            message: "Content must be a string".to_string(),
        })?;

        match encoding {
            Encoding::Base64 => STANDARD.decode(s).map_err(|e| Error::ImplementationFailure {
                message: format!("Invalid base64: {}", e),
            }),
            Encoding::Utf8 => Ok(s.as_bytes().to_vec()),
            Encoding::Latin1 => {
                // Validate that all chars are in Latin1 range
                let mut bytes = Vec::with_capacity(s.len());
                for c in s.chars() {
                    if c as u32 > 255 {
                        return Err(Error::ImplementationFailure {
                            message: format!("Character out of Latin1 range: {}", c),
                        });
                    }
                    bytes.push(c as u8);
                }
                Ok(bytes)
            }
            Encoding::Ascii => {
                for c in s.chars() {
                    if c as u32 > 127 {
                        return Err(Error::ImplementationFailure {
                            message: format!("Non-ASCII character: {}", c),
                        });
                    }
                }
                Ok(s.as_bytes().to_vec())
            }
        }
    }
}

impl Default for FsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct OpenRequest {
    path: String,
    #[serde(default)]
    mode: OpenMode,
    #[serde(default)]
    encoding: Encoding,
}

#[derive(Deserialize)]
struct PathRequest {
    path: String,
}

#[derive(Deserialize)]
struct RenameRequest {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct MkdirRequest {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
struct SeekRequest {
    #[serde(default)]
    pos: Option<u64>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    whence: Option<String>,
}

impl Reader for FsStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
        // Handle reading file content from handles
        if from.components.len() == 2 && from.components[0] == "handles" {
            let handle_id: u64 =
                from.components[1]
                    .parse()
                    .map_err(|_| Error::ImplementationFailure {
                        message: format!("Invalid handle ID: {}", from.components[1]),
                    })?;

            // Read file content and encoding, then encode
            let (buffer, encoding) = {
                let handle = self
                    .handles
                    .get_mut(&handle_id)
                    .ok_or_else(|| Error::ImplementationFailure {
                        message: format!("Handle {} not found", handle_id),
                    })?;

                let mut buffer = Vec::new();
                handle
                    .file
                    .read_to_end(&mut buffer)
                    .map_err(|e| Error::ImplementationFailure {
                        message: format!("Failed to read file: {}", e),
                    })?;
                (buffer, handle.encoding)
            };

            let content = self.encode_content(&buffer, encoding)?;
            return Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
                content,
            ))));
        }

        Ok(self.read_value(from)?.map(|v| {
            let de: Box<dyn erased_serde::Deserializer> =
                Box::new(<dyn erased_serde::Deserializer>::erase(v));
            de
        }))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error> {
        // Handle reading file content from handles
        if from.components.len() == 2 && from.components[0] == "handles" {
            let handle_id: u64 =
                from.components[1]
                    .parse()
                    .map_err(|_| Error::ImplementationFailure {
                        message: format!("Invalid handle ID: {}", from.components[1]),
                    })?;

            // Read file content and encoding, then encode
            let (buffer, encoding) = {
                let handle = self
                    .handles
                    .get_mut(&handle_id)
                    .ok_or_else(|| Error::ImplementationFailure {
                        message: format!("Handle {} not found", handle_id),
                    })?;

                let mut buffer = Vec::new();
                handle
                    .file
                    .read_to_end(&mut buffer)
                    .map_err(|e| Error::ImplementationFailure {
                        message: format!("Failed to read file: {}", e),
                    })?;
                (buffer, handle.encoding)
            };

            let content = self.encode_content(&buffer, encoding)?;
            let data: RecordType =
                serde_json::from_value(content).map_err(|err| Error::RecordDeserialization {
                    message: format!("Failed to deserialize file content: {}", err),
                })?;
            return Ok(Some(data));
        }

        if let Some(value) = self.read_value(from)? {
            let data: RecordType =
                serde_json::from_value(value).map_err(|err| Error::RecordDeserialization {
                    message: format!("Failed to deserialize fs value: {}", err),
                })?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

impl Writer for FsStore {
    fn write<RecordType: Serialize>(
        &mut self,
        path: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        if path.components.is_empty() {
            return Err(Error::ImplementationFailure {
                message: "Cannot write to fs root".to_string(),
            });
        }

        let value = serde_json::to_value(data).map_err(|err| Error::RecordSerialization {
            message: format!("Failed to serialize fs request: {}", err),
        })?;

        // Handle writes to handles
        if path.components[0] == "handles" {
            return self.write_handle(path, &value);
        }

        if path.components.len() != 1 {
            return Err(Error::ImplementationFailure {
                message: format!("Invalid fs path: {}", path),
            });
        }

        match path.components[0].as_str() {
            "open" => {
                let request: OpenRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid open request: {}", e),
                    })?;

                let file = match request.mode {
                    OpenMode::Read => File::open(&request.path),
                    OpenMode::Write => File::create(&request.path),
                    OpenMode::Append => OpenOptions::new()
                        .append(true)
                        .create(true)
                        .open(&request.path),
                    OpenMode::ReadWrite => OpenOptions::new()
                        .read(true)
                        .write(true)
                        .create(true)
                        .open(&request.path),
                    OpenMode::CreateNew => OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&request.path),
                }
                .map_err(|e| Error::ImplementationFailure {
                    message: format!("Failed to open '{}': {}", request.path, e),
                })?;

                let handle_id = Self::next_handle_id();
                self.handles.insert(
                    handle_id,
                    FileHandle {
                        file,
                        path: request.path,
                        mode: request.mode,
                        encoding: request.encoding,
                    },
                );

                // Return the handle path
                Ok(Path::parse(&format!("handles/{}", handle_id)).unwrap())
            }
            "stat" => {
                let request: PathRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid stat request: {}", e),
                    })?;

                let metadata =
                    fs::metadata(&request.path).map_err(|e| Error::ImplementationFailure {
                        message: format!("stat failed: {}", e),
                    })?;

                let file_type = if metadata.is_file() {
                    "file"
                } else if metadata.is_dir() {
                    "directory"
                } else if metadata.is_symlink() {
                    "symlink"
                } else {
                    "other"
                };

                #[cfg(unix)]
                let _result = json!({
                    "type": file_type,
                    "size": metadata.len(),
                    "mode": metadata.mode(),
                    "uid": metadata.uid(),
                    "gid": metadata.gid(),
                    "modified": metadata.mtime(),
                    "accessed": metadata.atime(),
                    "created": metadata.ctime(),
                });

                #[cfg(not(unix))]
                let _result = json!({
                    "type": file_type,
                    "size": metadata.len(),
                });

                Ok(path.clone())
            }
            "readdir" => {
                let request: PathRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid readdir request: {}", e),
                    })?;

                let _entries: Result<Vec<_>, _> = fs::read_dir(&request.path)
                    .map_err(|e| Error::ImplementationFailure {
                        message: format!("readdir failed: {}", e),
                    })?
                    .map(|entry| {
                        entry.map(|e| {
                            let path = e.path();
                            let name = e.file_name().to_string_lossy().to_string();
                            let is_dir = path.is_dir();
                            json!({
                                "name": name,
                                "type": if is_dir { "directory" } else { "file" }
                            })
                        })
                    })
                    .collect();

                Ok(path.clone())
            }
            "mkdir" => {
                let request: MkdirRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid mkdir request: {}", e),
                    })?;

                if request.recursive {
                    fs::create_dir_all(&request.path)
                } else {
                    fs::create_dir(&request.path)
                }
                .map_err(|e| Error::ImplementationFailure {
                    message: format!("mkdir failed: {}", e),
                })?;

                Ok(path.clone())
            }
            "rmdir" => {
                let request: PathRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid rmdir request: {}", e),
                    })?;

                fs::remove_dir(&request.path).map_err(|e| Error::ImplementationFailure {
                    message: format!("rmdir failed: {}", e),
                })?;

                Ok(path.clone())
            }
            "unlink" => {
                let request: PathRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid unlink request: {}", e),
                    })?;

                fs::remove_file(&request.path).map_err(|e| Error::ImplementationFailure {
                    message: format!("unlink failed: {}", e),
                })?;

                Ok(path.clone())
            }
            "rename" => {
                let request: RenameRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid rename request: {}", e),
                    })?;

                fs::rename(&request.from, &request.to).map_err(|e| Error::ImplementationFailure {
                    message: format!("rename failed: {}", e),
                })?;

                Ok(path.clone())
            }
            _ => Err(Error::ImplementationFailure {
                message: format!("Unknown fs operation: {}", path.components[0]),
            }),
        }
    }
}

impl FsStore {
    fn write_handle(&mut self, path: &Path, value: &JsonValue) -> Result<Path, Error> {
        if path.components.len() < 2 {
            return Err(Error::ImplementationFailure {
                message: "Invalid handle path".to_string(),
            });
        }

        let handle_id: u64 =
            path.components[1]
                .parse()
                .map_err(|_| Error::ImplementationFailure {
                    message: format!("Invalid handle ID: {}", path.components[1]),
                })?;

        // Handle close operation
        if path.components.len() == 3 && path.components[2] == "close" {
            self.handles
                .remove(&handle_id)
                .ok_or_else(|| Error::ImplementationFailure {
                    message: format!("Handle {} not found", handle_id),
                })?;
            return Ok(path.clone());
        }

        if path.components.len() == 2 {
            // Write content to file - get encoding first, then decode, then write
            let encoding = self
                .handles
                .get(&handle_id)
                .ok_or_else(|| Error::ImplementationFailure {
                    message: format!("Handle {} not found", handle_id),
                })?
                .encoding;

            let bytes = self.decode_content(value, encoding)?;

            let handle = self
                .handles
                .get_mut(&handle_id)
                .ok_or_else(|| Error::ImplementationFailure {
                    message: format!("Handle {} not found", handle_id),
                })?;

            handle
                .file
                .write_all(&bytes)
                .map_err(|e| Error::ImplementationFailure {
                    message: format!("Failed to write to file: {}", e),
                })?;
            return Ok(path.clone());
        }

        let handle = self
            .handles
            .get_mut(&handle_id)
            .ok_or_else(|| Error::ImplementationFailure {
                message: format!("Handle {} not found", handle_id),
            })?;

        match path.components[2].as_str() {
            "seek" => {
                let request: SeekRequest =
                    serde_json::from_value(value.clone()).map_err(|e| {
                        Error::ImplementationFailure {
                            message: format!("Invalid seek request: {}", e),
                        }
                    })?;

                let seek_pos = if let Some(pos) = request.pos {
                    SeekFrom::Start(pos)
                } else if let Some(offset) = request.offset {
                    match request.whence.as_deref() {
                        Some("end") => SeekFrom::End(offset),
                        Some("current") | None => SeekFrom::Current(offset),
                        Some("start") => SeekFrom::Start(offset as u64),
                        Some(w) => {
                            return Err(Error::ImplementationFailure {
                                message: format!("Invalid whence: {}", w),
                            })
                        }
                    }
                } else {
                    return Err(Error::ImplementationFailure {
                        message: "Seek requires 'pos' or 'offset'".to_string(),
                    });
                };

                handle.file.seek(seek_pos).map_err(|e| {
                    Error::ImplementationFailure {
                        message: format!("Seek failed: {}", e),
                    }
                })?;

                Ok(path.clone())
            }
            "close" => {
                self.handles.remove(&handle_id);
                Ok(path.clone())
            }
            op => Err(Error::ImplementationFailure {
                message: format!("Unknown handle operation: {}", op),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_mkdir_and_rmdir() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let new_dir = dir.path().join("test_dir");
        let path_str = new_dir.to_string_lossy().to_string();

        store
            .write(&Path::parse("mkdir").unwrap(), json!({"path": path_str}))
            .unwrap();
        assert!(new_dir.exists());

        store
            .write(&Path::parse("rmdir").unwrap(), json!({"path": path_str}))
            .unwrap();
        assert!(!new_dir.exists());
    }

    #[test]
    fn test_open_read_close() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Create file with content
        {
            let mut f = File::create(&file_path).unwrap();
            f.write_all(b"Hello, World!").unwrap();
        }

        // Open with UTF-8 encoding
        let handle_path = store
            .write(
                &Path::parse("open").unwrap(),
                json!({
                    "path": file_path.to_string_lossy(),
                    "mode": "read",
                    "encoding": "utf8"
                }),
            )
            .unwrap();

        assert!(handle_path.to_string().starts_with("handles/"));

        // Read content
        let content: String = store.read_owned(&handle_path).unwrap().unwrap();
        assert_eq!(content, "Hello, World!");

        // Close
        let close_path = handle_path.join(&Path::parse("close").unwrap());
        store.write(&close_path, json!(null)).unwrap();
    }

    #[test]
    fn test_open_write_base64() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("binary.bin");

        // Open for writing with base64 (default)
        let handle_path = store
            .write(
                &Path::parse("open").unwrap(),
                json!({
                    "path": file_path.to_string_lossy(),
                    "mode": "write"
                }),
            )
            .unwrap();

        // Write base64 content (binary data: 0x00 0x01 0x02)
        let content = STANDARD.encode([0u8, 1, 2, 255]);
        store.write(&handle_path, content).unwrap();

        // Close and verify
        let close_path = handle_path.join(&Path::parse("close").unwrap());
        store.write(&close_path, json!(null)).unwrap();

        let bytes = fs::read(&file_path).unwrap();
        assert_eq!(bytes, vec![0, 1, 2, 255]);
    }

    #[test]
    fn test_seek() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("seek_test.txt");

        // Create file
        fs::write(&file_path, "0123456789").unwrap();

        // Open for reading
        let handle_path = store
            .write(
                &Path::parse("open").unwrap(),
                json!({
                    "path": file_path.to_string_lossy(),
                    "encoding": "utf8"
                }),
            )
            .unwrap();

        // Read first 3 bytes
        // (For simplicity, we read all and check - real impl would support partial reads)

        // Seek to position 5
        let seek_path = handle_path.join(&Path::parse("seek").unwrap());
        store.write(&seek_path, json!({"pos": 5})).unwrap();

        // Read from position 5
        let content: String = store.read_owned(&handle_path).unwrap().unwrap();
        assert_eq!(content, "56789");
    }

    #[test]
    fn test_unlink() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");

        File::create(&file_path).unwrap();
        assert!(file_path.exists());

        let path_str = file_path.to_string_lossy().to_string();
        store
            .write(&Path::parse("unlink").unwrap(), json!({"path": path_str}))
            .unwrap();

        assert!(!file_path.exists());
    }

    #[test]
    fn test_rename() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let old_path = dir.path().join("old.txt");
        let new_path = dir.path().join("new.txt");

        File::create(&old_path).unwrap();
        assert!(old_path.exists());

        store
            .write(
                &Path::parse("rename").unwrap(),
                json!({
                    "from": old_path.to_string_lossy().to_string(),
                    "to": new_path.to_string_lossy().to_string()
                }),
            )
            .unwrap();

        assert!(!old_path.exists());
        assert!(new_path.exists());
    }
}
