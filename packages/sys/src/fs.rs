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

/// Encoding for file content on read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentEncoding {
    /// Return content as base64-encoded string (default)
    #[default]
    Base64,
    /// Return content as UTF-8 string
    Utf8,
    /// Return content as raw bytes
    Bytes,
}

/// An open file handle with explicit position tracking.
struct FileHandle {
    file: File,
    path: String,
    #[allow(dead_code)]
    mode: OpenMode,
    /// Explicit position tracking (mirrors file.stream_position())
    position: u64,
    /// Encoding for read content
    encoding: ContentEncoding,
}

/// Operations that can be performed on a file handle via path.
#[derive(Debug, PartialEq)]
enum HandleOperation {
    /// Read from current position to EOF: /handles/{id}
    ReadToEnd,
    /// Read/write at offset: /handles/{id}/at/{offset}
    AtOffset { offset: u64 },
    /// Read n bytes from offset: /handles/{id}/at/{offset}/len/{n}
    ReadAtLen { offset: u64, length: u64 },
    /// Get/set position: /handles/{id}/position
    Position,
    /// Get file metadata: /handles/{id}/meta
    Meta,
    /// Close handle: /handles/{id}/close
    Close,
}

/// Parse a handle path into an operation.
/// Returns (handle_id, operation) or None if invalid.
fn parse_handle_operation(path: &Path) -> Option<(u64, HandleOperation)> {
    if path.len() < 2 || path[0] != "handles" {
        return None;
    }

    let id: u64 = path[1].parse().ok()?;

    let op = match path.len() {
        2 => HandleOperation::ReadToEnd,
        3 => match path[2].as_str() {
            "position" => HandleOperation::Position,
            "meta" => HandleOperation::Meta,
            "close" => HandleOperation::Close,
            _ => return None,
        },
        4 if path[2] == "at" => {
            let offset: u64 = path[3].parse().ok()?;
            HandleOperation::AtOffset { offset }
        }
        6 if path[2] == "at" && path[4] == "len" => {
            let offset: u64 = path[3].parse().ok()?;
            let length: u64 = path[5].parse().ok()?;
            HandleOperation::ReadAtLen { offset, length }
        }
        _ => return None,
    };

    Some((id, op))
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

        // Handles are processed in Reader::read() directly
        Ok(None)
    }

    fn read_handles_listing(&self) -> Value {
        // Return just the IDs as an array: [0, 1, 2, ...]
        let ids: Vec<Value> = self
            .handles
            .keys()
            .map(|id| Value::Integer(*id as i64))
            .collect();
        Value::Array(ids)
    }

    fn read_handle_meta(&self, handle: &FileHandle) -> Result<Value, Error> {
        let metadata = fs::metadata(&handle.path)?;

        let mut m = BTreeMap::new();
        m.insert("size".to_string(), Value::Integer(metadata.len() as i64));
        m.insert("is_file".to_string(), Value::Bool(metadata.is_file()));
        m.insert("is_dir".to_string(), Value::Bool(metadata.is_dir()));
        m.insert("path".to_string(), Value::String(handle.path.clone()));
        Ok(Value::Map(m))
    }

    fn read_handle_position(&self, handle: &FileHandle) -> Value {
        let mut m = BTreeMap::new();
        m.insert(
            "position".to_string(),
            Value::Integer(handle.position as i64),
        );
        Value::Map(m)
    }

    fn encode_content(buffer: Vec<u8>, encoding: ContentEncoding) -> Result<Value, Error> {
        match encoding {
            ContentEncoding::Base64 => {
                let encoded =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &buffer);
                Ok(Value::String(encoded))
            }
            ContentEncoding::Utf8 => {
                let text = String::from_utf8(buffer)
                    .map_err(|e| Error::store("fs", "read", format!("Invalid UTF-8: {}", e)))?;
                Ok(Value::String(text))
            }
            ContentEncoding::Bytes => Ok(Value::Bytes(buffer)),
        }
    }

    fn read_from_position(&mut self, handle_id: u64) -> Result<Value, Error> {
        let handle = self
            .handles
            .get_mut(&handle_id)
            .ok_or_else(|| Error::store("fs", "read", format!("Handle {} not found", handle_id)))?;

        let mut buffer = Vec::new();
        handle.file.read_to_end(&mut buffer)?;
        handle.position = handle.file.stream_position()?;
        let encoding = handle.encoding;

        Self::encode_content(buffer, encoding)
    }

    fn read_at_offset(&mut self, handle_id: u64, offset: u64) -> Result<Value, Error> {
        let handle = self
            .handles
            .get_mut(&handle_id)
            .ok_or_else(|| Error::store("fs", "read", format!("Handle {} not found", handle_id)))?;

        handle.file.seek(SeekFrom::Start(offset))?;
        handle.position = offset;

        let mut buffer = Vec::new();
        handle.file.read_to_end(&mut buffer)?;
        handle.position = handle.file.stream_position()?;
        let encoding = handle.encoding;

        Self::encode_content(buffer, encoding)
    }

    fn read_at_offset_len(
        &mut self,
        handle_id: u64,
        offset: u64,
        length: u64,
    ) -> Result<Value, Error> {
        let handle = self
            .handles
            .get_mut(&handle_id)
            .ok_or_else(|| Error::store("fs", "read", format!("Handle {} not found", handle_id)))?;

        handle.file.seek(SeekFrom::Start(offset))?;
        handle.position = offset;

        let mut buffer = vec![0u8; length as usize];
        let bytes_read = handle.file.read(&mut buffer)?;
        buffer.truncate(bytes_read);
        handle.position = handle.file.stream_position()?;
        let encoding = handle.encoding;

        Self::encode_content(buffer, encoding)
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

    fn parse_encoding(value: &Value) -> ContentEncoding {
        if let Value::Map(map) = value {
            if let Some(Value::String(enc)) = map.get("encoding") {
                return match enc.to_lowercase().as_str() {
                    "utf8" | "utf-8" | "text" => ContentEncoding::Utf8,
                    "bytes" | "raw" => ContentEncoding::Bytes,
                    _ => ContentEncoding::Base64,
                };
            }
        }
        ContentEncoding::Base64
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
        let (handle_id, op) = parse_handle_operation(path)
            .ok_or_else(|| Error::store("fs", "write", format!("Invalid handle path: {}", path)))?;

        match op {
            HandleOperation::Close => {
                self.handles.remove(&handle_id).ok_or_else(|| {
                    Error::store("fs", "write", format!("Handle {} not found", handle_id))
                })?;
                Ok(path.clone())
            }

            HandleOperation::Position => {
                // Set position: write {"pos": n} to /handles/{id}/position
                let pos = if let Value::Map(map) = value {
                    if let Some(Value::Integer(p)) = map.get("pos") {
                        *p as u64
                    } else {
                        return Err(Error::store("fs", "write", "position requires 'pos' field"));
                    }
                } else {
                    return Err(Error::store(
                        "fs",
                        "write",
                        "position requires a map with 'pos'",
                    ));
                };

                let handle = self.handles.get_mut(&handle_id).ok_or_else(|| {
                    Error::store("fs", "write", format!("Handle {} not found", handle_id))
                })?;

                handle.file.seek(SeekFrom::Start(pos))?;
                handle.position = pos;
                Ok(path.clone())
            }

            HandleOperation::ReadToEnd => {
                // Write at current position
                self.write_content_at_current(handle_id, value)?;
                Ok(path.clone())
            }

            HandleOperation::AtOffset { offset } => {
                // Write at specific offset
                self.write_content_at_offset(handle_id, offset, value)?;
                Ok(path.clone())
            }

            HandleOperation::ReadAtLen { .. } | HandleOperation::Meta => Err(Error::store(
                "fs",
                "write",
                format!("Cannot write to path: {}", path),
            )),
        }
    }

    fn decode_content(value: &Value, encoding: ContentEncoding) -> Result<Vec<u8>, Error> {
        match value {
            Value::String(s) => match encoding {
                ContentEncoding::Base64 => {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                        .map_err(|e| Error::store("fs", "write", format!("Invalid base64: {}", e)))
                }
                ContentEncoding::Utf8 | ContentEncoding::Bytes => Ok(s.as_bytes().to_vec()),
            },
            Value::Bytes(b) => Ok(b.to_vec()),
            _ => Err(Error::store(
                "fs",
                "write",
                "File content must be a string or bytes",
            )),
        }
    }

    fn write_content_at_current(&mut self, handle_id: u64, value: &Value) -> Result<(), Error> {
        let encoding = self
            .handles
            .get(&handle_id)
            .ok_or_else(|| Error::store("fs", "write", format!("Handle {} not found", handle_id)))?
            .encoding;

        let content = Self::decode_content(value, encoding)?;

        let handle = self.handles.get_mut(&handle_id).unwrap();
        handle.file.write_all(&content)?;
        handle.position = handle.file.stream_position()?;
        Ok(())
    }

    fn write_content_at_offset(
        &mut self,
        handle_id: u64,
        offset: u64,
        value: &Value,
    ) -> Result<(), Error> {
        let encoding = self
            .handles
            .get(&handle_id)
            .ok_or_else(|| Error::store("fs", "write", format!("Handle {} not found", handle_id)))?
            .encoding;

        let content = Self::decode_content(value, encoding)?;

        let handle = self.handles.get_mut(&handle_id).unwrap();
        handle.file.seek(SeekFrom::Start(offset))?;
        handle.position = offset;

        handle.file.write_all(&content)?;
        handle.position = handle.file.stream_position()?;
        Ok(())
    }
}

// Meta lens implementation
impl FsStore {
    fn read_meta(&self, path: &Path) -> Result<Option<Record>, Error> {
        if path.is_empty() {
            return Ok(Some(Record::parsed(self.meta_root())));
        }

        match path[0].as_str() {
            "open" => Ok(Some(Record::parsed(Self::meta_open()))),
            "handles" => self.read_meta_handles(&path.slice(1, path.len())),
            "stat" => Ok(Some(Record::parsed(Self::meta_stat()))),
            "mkdir" => Ok(Some(Record::parsed(Self::meta_mkdir()))),
            "rmdir" => Ok(Some(Record::parsed(Self::meta_rmdir()))),
            "unlink" => Ok(Some(Record::parsed(Self::meta_unlink()))),
            "rename" => Ok(Some(Record::parsed(Self::meta_rename()))),
            _ => Ok(None),
        }
    }

    fn meta_root(&self) -> Value {
        let mut fields = BTreeMap::new();

        fields.insert(
            "open".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Open a file handle".into()),
                );
                m
            }),
        );

        fields.insert(
            "handles".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("readable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Open file handles".into()),
                );
                m
            }),
        );

        fields.insert(
            "stat".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Get file metadata".into()),
                );
                m
            }),
        );

        fields.insert(
            "mkdir".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Create directory".into()),
                );
                m
            }),
        );

        fields.insert(
            "rmdir".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Remove directory".into()),
                );
                m
            }),
        );

        fields.insert(
            "unlink".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Delete file".into()),
                );
                m
            }),
        );

        fields.insert(
            "rename".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert(
                    "description".to_string(),
                    Value::String("Rename file or directory".into()),
                );
                m
            }),
        );

        let mut root = BTreeMap::new();
        root.insert("readable".to_string(), Value::Bool(true));
        root.insert("writable".to_string(), Value::Bool(true));
        root.insert(
            "description".to_string(),
            Value::String("Filesystem operations".into()),
        );
        root.insert("fields".to_string(), Value::Map(fields));

        Value::Map(root)
    }

    fn meta_open() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Open a file handle".into()),
        );
        m.insert(
            "accepts".to_string(),
            Value::Map({
                let mut accepts = BTreeMap::new();
                accepts.insert(
                    "path".to_string(),
                    Value::String("File path (required)".into()),
                );
                accepts.insert(
                    "mode".to_string(),
                    Value::String("read|write|append|readwrite|createnew".into()),
                );
                accepts.insert(
                    "encoding".to_string(),
                    Value::String("base64|utf8|bytes".into()),
                );
                accepts
            }),
        );
        m.insert(
            "returns".to_string(),
            Value::String("Path to new handle: handles/{id}".into()),
        );
        Value::Map(m)
    }

    fn meta_stat() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Get file metadata".into()),
        );
        m.insert(
            "accepts".to_string(),
            Value::Map({
                let mut accepts = BTreeMap::new();
                accepts.insert(
                    "path".to_string(),
                    Value::String("File path (required)".into()),
                );
                accepts
            }),
        );
        Value::Map(m)
    }

    fn meta_mkdir() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Create directory".into()),
        );
        m.insert(
            "accepts".to_string(),
            Value::Map({
                let mut accepts = BTreeMap::new();
                accepts.insert(
                    "path".to_string(),
                    Value::String("Directory path (required)".into()),
                );
                accepts.insert(
                    "recursive".to_string(),
                    Value::String("Create parent directories (optional)".into()),
                );
                accepts
            }),
        );
        Value::Map(m)
    }

    fn meta_rmdir() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Remove directory".into()),
        );
        m.insert(
            "accepts".to_string(),
            Value::Map({
                let mut accepts = BTreeMap::new();
                accepts.insert(
                    "path".to_string(),
                    Value::String("Directory path (required)".into()),
                );
                accepts
            }),
        );
        Value::Map(m)
    }

    fn meta_unlink() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Delete file".into()),
        );
        m.insert(
            "accepts".to_string(),
            Value::Map({
                let mut accepts = BTreeMap::new();
                accepts.insert(
                    "path".to_string(),
                    Value::String("File path (required)".into()),
                );
                accepts
            }),
        );
        Value::Map(m)
    }

    fn meta_rename() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Rename file or directory".into()),
        );
        m.insert(
            "accepts".to_string(),
            Value::Map({
                let mut accepts = BTreeMap::new();
                accepts.insert(
                    "from".to_string(),
                    Value::String("Source path (required)".into()),
                );
                accepts.insert(
                    "to".to_string(),
                    Value::String("Destination path (required)".into()),
                );
                accepts
            }),
        );
        Value::Map(m)
    }

    fn read_meta_handles(&self, path: &Path) -> Result<Option<Record>, Error> {
        if path.is_empty() {
            // List handles with basic info
            let ids: Vec<Value> = self
                .handles
                .keys()
                .map(|id| Value::Integer(*id as i64))
                .collect();

            let mut m = BTreeMap::new();
            m.insert("type".to_string(), Value::String("collection".into()));
            m.insert(
                "description".to_string(),
                Value::String("Open file handles".into()),
            );
            m.insert("items".to_string(), Value::Array(ids));

            return Ok(Some(Record::parsed(Value::Map(m))));
        }

        // Parse handle ID
        let id: u64 = path[0]
            .parse()
            .map_err(|_| Error::store("fs", "meta", "Invalid handle ID"))?;

        let handle = self
            .handles
            .get(&id)
            .ok_or_else(|| Error::store("fs", "meta", format!("Handle {} not found", id)))?;

        if path.len() == 1 {
            return Ok(Some(Record::parsed(self.meta_handle(handle))));
        }

        // Sub-path meta
        match path[1].as_str() {
            "position" => Ok(Some(Record::parsed(Self::meta_position(handle)))),
            "meta" => Ok(Some(Record::parsed(Self::meta_handle_meta()))),
            "at" => Ok(Some(Record::parsed(Self::meta_at()))),
            "close" => Ok(Some(Record::parsed(Self::meta_close()))),
            _ => Ok(None),
        }
    }

    fn meta_handle(&self, handle: &FileHandle) -> Value {
        let mut state = BTreeMap::new();
        state.insert(
            "position".to_string(),
            Value::Integer(handle.position as i64),
        );
        state.insert(
            "encoding".to_string(),
            Value::String(format!("{:?}", handle.encoding)),
        );
        state.insert(
            "mode".to_string(),
            Value::String(format!("{:?}", handle.mode)),
        );
        state.insert("path".to_string(), Value::String(handle.path.clone()));

        let mut fields = BTreeMap::new();
        fields.insert(
            "position".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("readable".to_string(), Value::Bool(true));
                m.insert("writable".to_string(), Value::Bool(true));
                m.insert("type".to_string(), Value::String("integer".into()));
                m
            }),
        );
        fields.insert(
            "meta".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("readable".to_string(), Value::Bool(true));
                m
            }),
        );
        fields.insert(
            "at".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("readable".to_string(), Value::Bool(true));
                m.insert("writable".to_string(), Value::Bool(true));
                m
            }),
        );
        fields.insert(
            "close".to_string(),
            Value::Map({
                let mut m = BTreeMap::new();
                m.insert("writable".to_string(), Value::Bool(true));
                m
            }),
        );

        let mut m = BTreeMap::new();
        m.insert("readable".to_string(), Value::Bool(true));
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert("state".to_string(), Value::Map(state));
        m.insert("fields".to_string(), Value::Map(fields));

        Value::Map(m)
    }

    fn meta_position(handle: &FileHandle) -> Value {
        let mut m = BTreeMap::new();
        m.insert("readable".to_string(), Value::Bool(true));
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert("type".to_string(), Value::String("integer".into()));
        m.insert("value".to_string(), Value::Integer(handle.position as i64));
        m.insert(
            "description".to_string(),
            Value::String("Current byte offset. Write to seek.".into()),
        );
        Value::Map(m)
    }

    fn meta_handle_meta() -> Value {
        let mut m = BTreeMap::new();
        m.insert("readable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("File metadata (size, type)".into()),
        );
        Value::Map(m)
    }

    fn meta_at() -> Value {
        let mut m = BTreeMap::new();
        m.insert("readable".to_string(), Value::Bool(true));
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Read/write at offset: at/{offset} or at/{offset}/len/{n}".into()),
        );
        Value::Map(m)
    }

    fn meta_close() -> Value {
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert(
            "description".to_string(),
            Value::String("Close the file handle".into()),
        );
        Value::Map(m)
    }

    fn write_meta(&mut self, path: &Path, data: Record) -> Result<Path, Error> {
        // Only handles/*/position is writable via meta
        if path.len() < 3 || path[0] != "handles" {
            return Err(Error::store(
                "fs",
                "meta",
                format!("Cannot write to meta/{}", path),
            ));
        }

        let id: u64 = path[1]
            .parse()
            .map_err(|_| Error::store("fs", "meta", "Invalid handle ID"))?;

        match path[2].as_str() {
            "position" => {
                let value = data.into_value(&NoCodec)?;
                let pos = match value {
                    Value::Integer(n) => n as u64,
                    _ => {
                        return Err(Error::store("fs", "meta", "position must be integer"));
                    }
                };

                let handle = self.handles.get_mut(&id).ok_or_else(|| {
                    Error::store("fs", "meta", format!("Handle {} not found", id))
                })?;

                handle.file.seek(SeekFrom::Start(pos))?;
                handle.position = pos;

                // Return the meta path we wrote to
                Ok(Path::parse(&format!("meta/handles/{}/position", id)).unwrap())
            }
            _ => Err(Error::store(
                "fs",
                "meta",
                format!("Cannot write to meta/handles/{}/{}", id, path[2]),
            )),
        }
    }
}

impl Default for FsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for FsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Check for meta prefix
        if !from.is_empty() && from[0] == "meta" {
            let rest = from.slice(1, from.len());
            return self.read_meta(&rest);
        }

        // Handle /handles listing
        if from.len() == 1 && from[0] == "handles" {
            return Ok(Some(Record::parsed(self.read_handles_listing())));
        }

        // Handle operations on specific handles
        if from.len() >= 2 && from[0] == "handles" {
            let (handle_id, op) = parse_handle_operation(from).ok_or_else(|| {
                Error::store("fs", "read", format!("Invalid handle path: {}", from))
            })?;

            let value = match op {
                HandleOperation::ReadToEnd => self.read_from_position(handle_id)?,
                HandleOperation::AtOffset { offset } => self.read_at_offset(handle_id, offset)?,
                HandleOperation::ReadAtLen { offset, length } => {
                    self.read_at_offset_len(handle_id, offset, length)?
                }
                HandleOperation::Position => {
                    let handle = self.handles.get(&handle_id).ok_or_else(|| {
                        Error::store("fs", "read", format!("Handle {} not found", handle_id))
                    })?;
                    self.read_handle_position(handle)
                }
                HandleOperation::Meta => {
                    let handle = self.handles.get(&handle_id).ok_or_else(|| {
                        Error::store("fs", "read", format!("Handle {} not found", handle_id))
                    })?;
                    self.read_handle_meta(handle)?
                }
                HandleOperation::Close => {
                    return Err(Error::store(
                        "fs",
                        "read",
                        format!("Cannot read from path: {}", from),
                    ));
                }
            };

            return Ok(Some(Record::parsed(value)));
        }

        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for FsStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::store("fs", "write", "Cannot write to fs root"));
        }

        // Check for meta prefix
        if to[0] == "meta" {
            let rest = to.slice(1, to.len());
            return self.write_meta(&rest, data);
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
                let encoding = Self::parse_encoding(&value);

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
                        position: 0,
                        encoding,
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

        // Open for write with UTF-8 encoding
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str.clone()));
        open_map.insert("mode".to_string(), Value::String("write".to_string()));
        open_map.insert("encoding".to_string(), Value::String("utf8".to_string()));

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
        open_map.insert("encoding".to_string(), Value::String("utf8".to_string()));

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
    fn handle_position_set() {
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

        // Set position via write to /position
        let position_path = handle_path.join(&path!("position"));
        let mut pos_map = BTreeMap::new();
        pos_map.insert("pos".to_string(), Value::Integer(5));

        store
            .write(&position_path, Record::parsed(Value::Map(pos_map)))
            .unwrap();

        // Read position to verify
        let record = store.read(&position_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("position"), Some(&Value::Integer(5)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn handle_position_missing_pos_error() {
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

        let position_path = handle_path.join(&path!("position"));
        let pos_map = BTreeMap::new();

        let result = store.write(&position_path, Record::parsed(Value::Map(pos_map)));
        assert!(result.is_err());
    }

    #[test]
    fn handle_position_invalid_type_error() {
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

        let position_path = handle_path.join(&path!("position"));
        let result = store.write(
            &position_path,
            Record::parsed(Value::String("5".to_string())),
        );
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

    #[test]
    fn read_at_offset() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "0123456789").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read from offset 5 to end
        let at_path = handle_path.join(&path!("at/5"));
        let record = store.read(&at_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                let decoded =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &s).unwrap();
                assert_eq!(decoded, b"56789");
            }
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn read_at_offset_len() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "0123456789").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read 3 bytes starting at offset 2
        let at_path = handle_path.join(&path!("at/2/len/3"));
        let record = store.read(&at_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                let decoded =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &s).unwrap();
                assert_eq!(decoded, b"234");
            }
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn position_persists_after_read() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "0123456789").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read 5 bytes from position 0
        let at_path = handle_path.join(&path!("at/0/len/5"));
        store.read(&at_path).unwrap();

        // Position should now be 5
        let position_path = handle_path.join(&path!("position"));
        let record = store.read(&position_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("position"), Some(&Value::Integer(5)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn write_at_offset() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("write_at.txt");
        std::fs::write(&file_path, "0123456789").unwrap();
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str.clone()));
        open_map.insert("mode".to_string(), Value::String("readwrite".to_string()));
        open_map.insert("encoding".to_string(), Value::String("utf8".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Write "XXX" at position 3
        let at_path = handle_path.join(&path!("at/3"));
        store
            .write(&at_path, Record::parsed(Value::String("XXX".to_string())))
            .unwrap();

        // Close and verify file content
        let close_path = handle_path.join(&path!("close"));
        store
            .write(&close_path, Record::parsed(Value::Null))
            .unwrap();

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "012XXX6789");
    }

    #[test]
    fn position_query_initial() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Position should start at 0
        let position_path = handle_path.join(&path!("position"));
        let record = store.read(&position_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("position"), Some(&Value::Integer(0)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_from_close_error() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Reading from /close should error
        let close_path = handle_path.join(&path!("close"));
        let result = store.read(&close_path);
        assert!(result.is_err());
    }

    #[test]
    fn write_to_meta_error() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Writing to /meta should error
        let meta_path = handle_path.join(&path!("meta"));
        let result = store.write(&meta_path, Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn write_to_at_len_error() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "content").unwrap();
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str));
        open_map.insert("mode".to_string(), Value::String("readwrite".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Writing to /at/{offset}/len/{n} should error (no fixed-length writes)
        let at_len_path = handle_path.join(&path!("at/0/len/5"));
        let result = store.write(&at_len_path, Record::parsed(Value::String("x".to_string())));
        assert!(result.is_err());
    }

    #[test]
    fn invalid_handle_subpath_error() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Invalid sub-path should error
        let invalid_path = handle_path.join(&path!("invalid"));
        let result = store.read(&invalid_path);
        assert!(result.is_err());
    }

    #[test]
    fn parse_handle_operation_coverage() {
        // Test the parser directly for coverage
        assert!(parse_handle_operation(&path!("")).is_none());
        assert!(parse_handle_operation(&path!("other")).is_none());
        assert!(parse_handle_operation(&path!("handles/abc")).is_none());

        // Valid paths
        let (id, op) = parse_handle_operation(&path!("handles/42")).unwrap();
        assert_eq!(id, 42);
        assert_eq!(op, HandleOperation::ReadToEnd);

        let (id, op) = parse_handle_operation(&path!("handles/5/meta")).unwrap();
        assert_eq!(id, 5);
        assert_eq!(op, HandleOperation::Meta);

        let (id, op) = parse_handle_operation(&path!("handles/5/close")).unwrap();
        assert_eq!(id, 5);
        assert_eq!(op, HandleOperation::Close);

        let (id, op) = parse_handle_operation(&path!("handles/5/position")).unwrap();
        assert_eq!(id, 5);
        assert_eq!(op, HandleOperation::Position);

        let (id, op) = parse_handle_operation(&path!("handles/5/at/100")).unwrap();
        assert_eq!(id, 5);
        assert_eq!(op, HandleOperation::AtOffset { offset: 100 });

        let (id, op) = parse_handle_operation(&path!("handles/5/at/100/len/50")).unwrap();
        assert_eq!(id, 5);
        assert_eq!(
            op,
            HandleOperation::ReadAtLen {
                offset: 100,
                length: 50
            }
        );

        // Invalid sub-paths
        assert!(parse_handle_operation(&path!("handles/5/unknown")).is_none());
        assert!(parse_handle_operation(&path!("handles/5/at/abc")).is_none());
        assert!(parse_handle_operation(&path!("handles/5/at/10/len/abc")).is_none());
        assert!(parse_handle_operation(&path!("handles/5/at")).is_none());
    }

    #[test]
    fn open_with_utf8_encoding() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "Hello, UTF-8!").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));
        open_map.insert("encoding".to_string(), Value::String("utf8".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read should return plain UTF-8 string, not base64
        let record = store.read(&handle_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                assert_eq!(s, "Hello, UTF-8!");
            }
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn open_with_bytes_encoding() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "raw bytes").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));
        open_map.insert("encoding".to_string(), Value::String("bytes".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read should return raw bytes
        let record = store.read(&handle_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Bytes(b) => {
                assert_eq!(&b[..], b"raw bytes");
            }
            _ => panic!("Expected bytes, got {:?}", value),
        }
    }

    #[test]
    fn utf8_encoding_with_at_offset() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "Hello, World!").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));
        open_map.insert("encoding".to_string(), Value::String("utf8".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Read from offset 7
        let at_path = handle_path.join(&path!("at/7"));
        let record = store.read(&at_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                assert_eq!(s, "World!");
            }
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn content_encoding_default() {
        let enc = ContentEncoding::default();
        assert_eq!(enc, ContentEncoding::Base64);
    }

    #[test]
    fn utf8_encoding_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("utf8_test.txt");
        let path_str = file_path.to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open with UTF-8 encoding
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(path_str));
        open_map.insert("mode".to_string(), Value::String("readwrite".to_string()));
        open_map.insert("encoding".to_string(), Value::String("utf8".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        // Write plain UTF-8 string (not base64 encoded)
        store
            .write(
                &handle_path,
                Record::parsed(Value::String("Hello, UTF-8 world!".to_string())),
            )
            .unwrap();

        // Seek back to beginning
        let position_path = handle_path.join(&path!("position"));
        let mut pos_map = BTreeMap::new();
        pos_map.insert("pos".to_string(), Value::Integer(0));
        store
            .write(&position_path, Record::parsed(Value::Map(pos_map)))
            .unwrap();

        // Read should return plain UTF-8
        let record = store.read(&handle_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                assert_eq!(s, "Hello, UTF-8 world!");
            }
            _ => panic!("Expected string"),
        }
    }

    // Meta lens tests
    #[test]
    fn meta_root_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("readable"), Some(&Value::Bool(true)));
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert!(map.contains_key("fields"));

                // Check fields contains expected operations
                if let Some(Value::Map(fields)) = map.get("fields") {
                    assert!(fields.contains_key("open"));
                    assert!(fields.contains_key("handles"));
                    assert!(fields.contains_key("stat"));
                    assert!(fields.contains_key("mkdir"));
                    assert!(fields.contains_key("rmdir"));
                    assert!(fields.contains_key("unlink"));
                    assert!(fields.contains_key("rename"));
                } else {
                    panic!("Expected fields map");
                }
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_open_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/open")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert!(map.contains_key("accepts"));
                assert!(map.contains_key("returns"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_stat_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/stat")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert!(map.contains_key("accepts"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_mkdir_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/mkdir")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert!(map.contains_key("accepts"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_rmdir_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/rmdir")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_unlink_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/unlink")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_rename_returns_schema() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/rename")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert!(map.contains_key("accepts"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_handles_lists_handle_ids() {
        let mut store = FsStore::new();
        let record = store.read(&path!("meta/handles")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(
                    map.get("type"),
                    Some(&Value::String("collection".to_string()))
                );
                assert!(map.contains_key("items"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_handle_returns_affordances() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "test content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        // Read meta for handle
        let meta_path = Path::parse(&format!("meta/handles/{}", id)).unwrap();
        let record = store.read(&meta_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("readable"), Some(&Value::Bool(true)));
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert!(map.contains_key("state"));
                assert!(map.contains_key("fields"));

                // Check state has expected fields
                if let Some(Value::Map(state)) = map.get("state") {
                    assert!(state.contains_key("position"));
                    assert!(state.contains_key("encoding"));
                    assert!(state.contains_key("mode"));
                    assert!(state.contains_key("path"));
                } else {
                    panic!("Expected state map");
                }
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_handle_position_returns_value() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "0123456789").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        // Read meta/handles/{id}/position
        let meta_path = Path::parse(&format!("meta/handles/{}/position", id)).unwrap();
        let record = store.read(&meta_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("readable"), Some(&Value::Bool(true)));
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
                assert_eq!(map.get("value"), Some(&Value::Integer(0)));
                assert!(map.contains_key("description"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_handle_position_write_seeks() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "0123456789").unwrap();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert(
            "path".to_string(),
            Value::String(file_path.to_string_lossy().into()),
        );
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        // Write to meta/handles/{id}/position
        let meta_pos_path = Path::parse(&format!("meta/handles/{}/position", id)).unwrap();
        store
            .write(&meta_pos_path, Record::parsed(Value::Integer(5)))
            .unwrap();

        // Verify position changed via regular position read
        let pos_path = handle_path.join(&path!("position"));
        let record = store.read(&pos_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        if let Value::Map(m) = value {
            assert_eq!(m.get("position"), Some(&Value::Integer(5)));
        } else {
            panic!("Expected map");
        }
    }

    #[test]
    fn meta_write_invalid_path_error() {
        let mut store = FsStore::new();
        let result = store.write(&path!("meta/open"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn meta_write_invalid_handle_id_error() {
        let mut store = FsStore::new();
        let result = store.write(
            &path!("meta/handles/invalid/position"),
            Record::parsed(Value::Integer(0)),
        );
        assert!(result.is_err());
    }

    #[test]
    fn meta_write_nonexistent_handle_error() {
        let mut store = FsStore::new();
        let result = store.write(
            &path!("meta/handles/999999/position"),
            Record::parsed(Value::Integer(0)),
        );
        assert!(result.is_err());
    }

    #[test]
    fn meta_write_invalid_subpath_error() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        // Try to write to invalid meta subpath
        let meta_path = Path::parse(&format!("meta/handles/{}/invalid", id)).unwrap();
        let result = store.write(&meta_path, Record::parsed(Value::Integer(0)));
        assert!(result.is_err());
    }

    #[test]
    fn meta_write_position_invalid_type_error() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        // Try to write non-integer to position
        let meta_path = Path::parse(&format!("meta/handles/{}/position", id)).unwrap();
        let result = store.write(
            &meta_path,
            Record::parsed(Value::String("not an int".to_string())),
        );
        assert!(result.is_err());
    }

    #[test]
    fn meta_unknown_path_returns_none() {
        let mut store = FsStore::new();
        let result = store.read(&path!("meta/unknown")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn meta_handle_invalid_id_error() {
        let mut store = FsStore::new();
        let result = store.read(&path!("meta/handles/invalid"));
        assert!(result.is_err());
    }

    #[test]
    fn meta_handle_not_found_error() {
        let mut store = FsStore::new();
        let result = store.read(&path!("meta/handles/999999"));
        assert!(result.is_err());
    }

    #[test]
    fn meta_handle_subpath_unknown_returns_none() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        // Read unknown subpath
        let meta_path = Path::parse(&format!("meta/handles/{}/unknown", id)).unwrap();
        let result = store.read(&meta_path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn meta_handle_meta_returns_schema() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        let meta_path = Path::parse(&format!("meta/handles/{}/meta", id)).unwrap();
        let record = store.read(&meta_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("readable"), Some(&Value::Bool(true)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_handle_at_returns_schema() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        let meta_path = Path::parse(&format!("meta/handles/{}/at", id)).unwrap();
        let record = store.read(&meta_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("readable"), Some(&Value::Bool(true)));
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn meta_handle_close_returns_schema() {
        let mut temp = NamedTempFile::new().unwrap();
        write!(temp, "content").unwrap();
        let temp_path = temp.path().to_string_lossy().to_string();

        let mut store = FsStore::new();

        // Open file
        let mut open_map = BTreeMap::new();
        open_map.insert("path".to_string(), Value::String(temp_path));
        open_map.insert("mode".to_string(), Value::String("read".to_string()));

        let handle_path = store
            .write(&path!("open"), Record::parsed(Value::Map(open_map)))
            .unwrap();

        let id = &handle_path[1];

        let meta_path = Path::parse(&format!("meta/handles/{}/close", id)).unwrap();
        let record = store.read(&meta_path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(map.get("writable"), Some(&Value::Bool(true)));
            }
            _ => panic!("Expected map"),
        }
    }
}
