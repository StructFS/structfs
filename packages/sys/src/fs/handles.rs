//! The open-file handle table: `handles/{id}` and its sub-paths.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

use collection_literals::btree;
use structfs_core_store::{path, Error, Path, Reference, Value};

use super::encoding::{non_negative, ContentEncoding, OpenMode};
use super::FsStore;

/// An open file. The file's own cursor is the single source of truth for
/// the position.
pub(super) struct FileHandle {
    pub(super) file: File,
    /// The OS path the handle was opened with (after confinement).
    pub(super) path: String,
    pub(super) mode: OpenMode,
    pub(super) encoding: ContentEncoding,
}

/// Operations addressed by a handle path.
#[derive(Debug, PartialEq)]
pub(super) enum HandleOperation {
    /// `handles/{id}`: read to EOF from the cursor / write at the cursor.
    Cursor,
    /// `handles/{id}/at/{offset}`: read to EOF / write from an offset.
    AtOffset { offset: u64 },
    /// `handles/{id}/at/{offset}/len/{n}`: read at most `n` bytes.
    ReadAtLen { offset: u64, length: u64 },
    /// `handles/{id}/position`: read or set the cursor.
    Position,
    /// `handles/{id}/meta`: file metadata.
    Meta,
    /// `handles/{id}/close`: write anything to close.
    Close,
}

/// Parse `handles/{id}[/...]` into `(id, operation)`.
pub(super) fn parse_handle_operation(path: &Path) -> Option<(u64, HandleOperation)> {
    if path.len() < 2 || &path[0] != "handles" {
        return None;
    }
    let id: u64 = path[1].parse().ok()?;
    let op = match path.len() {
        2 => HandleOperation::Cursor,
        3 => match &path[2] {
            "position" => HandleOperation::Position,
            "meta" => HandleOperation::Meta,
            "close" => HandleOperation::Close,
            _ => return None,
        },
        4 if &path[2] == "at" => HandleOperation::AtOffset {
            offset: path[3].parse().ok()?,
        },
        6 if &path[2] == "at" && &path[4] == "len" => HandleOperation::ReadAtLen {
            offset: path[3].parse().ok()?,
            length: path[5].parse().ok()?,
        },
        _ => return None,
    };
    Some((id, op))
}

/// The data path of handle `id`.
pub(super) fn handle_path(id: u64) -> Path {
    let mut path = path!("handles");
    path.push(id);
    path
}

impl FsStore {
    fn handle_mut(&mut self, id: u64) -> Result<&mut FileHandle, Error> {
        self.handles
            .get_mut(&id)
            .ok_or_else(|| Error::not_found(handle_path(id)))
    }

    pub(super) fn handle(&self, id: u64) -> Result<&FileHandle, Error> {
        self.handles
            .get(&id)
            .ok_or_else(|| Error::not_found(handle_path(id)))
    }

    /// `read handles`: the open handles, in id order.
    pub(super) fn handles_listing(&self) -> Value {
        let items = self
            .handles
            .keys()
            .map(|id| Reference::with_type(format!("handles/{id}"), "handle").to_value())
            .collect();
        Value::Map(btree! { "items".into() => Value::Array(items) })
    }

    /// Read at most `limit` bytes from the cursor; more is `ResourceLimit`.
    fn read_capped(file: &mut File, limit: u64) -> Result<Vec<u8>, Error> {
        let mut buffer = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut buffer)?;
        if buffer.len() as u64 > limit {
            return Err(Error::resource_limit(format!(
                "read exceeds the {limit}-byte limit; page with at/{{offset}}/len/{{n}}"
            )));
        }
        Ok(buffer)
    }

    pub(super) fn read_handle(&mut self, id: u64, op: HandleOperation) -> Result<Value, Error> {
        let limit = self.max_read_len;
        let handle = self.handle_mut(id)?;
        match op {
            HandleOperation::Cursor => {
                let buffer = Self::read_capped(&mut handle.file, limit)?;
                handle.encoding.encode(buffer)
            }
            HandleOperation::AtOffset { offset } => {
                handle.file.seek(SeekFrom::Start(offset))?;
                let buffer = Self::read_capped(&mut handle.file, limit)?;
                handle.encoding.encode(buffer)
            }
            HandleOperation::ReadAtLen { offset, length } => {
                if length > limit {
                    return Err(Error::resource_limit(format!(
                        "requested {length} bytes; the read limit is {limit}"
                    )));
                }
                handle.file.seek(SeekFrom::Start(offset))?;
                let mut buffer = Vec::new();
                (&mut handle.file).take(length).read_to_end(&mut buffer)?;
                handle.encoding.encode(buffer)
            }
            HandleOperation::Position => {
                let position = handle.file.stream_position()?;
                Ok(Value::Map(btree! {
                    "position".into() => Value::Integer(position as i64),
                }))
            }
            HandleOperation::Meta => {
                let metadata = handle.file.metadata()?;
                Ok(Value::Map(btree! {
                    "size".into() => Value::Integer(metadata.len() as i64),
                    "is_file".into() => Value::Bool(metadata.is_file()),
                    "is_dir".into() => Value::Bool(metadata.is_dir()),
                    "path".into() => Value::String(handle.path.clone()),
                }))
            }
            HandleOperation::Close => {
                Err(Error::permission_denied("handles/{id}/close is write-only"))
            }
        }
    }

    pub(super) fn write_handle(&mut self, path: &Path, value: &Value) -> Result<Path, Error> {
        let (id, op) = parse_handle_operation(path)
            .ok_or_else(|| Error::invalid_argument(format!("invalid handle path: {path}")))?;
        match op {
            // Null on the handle itself deletes it (the Null-as-deletion
            // convention): the same as `close`.
            HandleOperation::Cursor if value.is_null() => {
                self.handles
                    .remove(&id)
                    .ok_or_else(|| Error::not_found(handle_path(id)))?;
            }
            HandleOperation::Close => {
                self.handles
                    .remove(&id)
                    .ok_or_else(|| Error::not_found(handle_path(id)))?;
            }
            HandleOperation::Position => {
                let pos = match value {
                    Value::Map(map) => map
                        .get("pos")
                        .ok_or_else(|| Error::invalid_argument("position requires a 'pos' field"))
                        .and_then(|pos| non_negative(pos, "pos"))?,
                    _ => {
                        return Err(Error::invalid_argument(
                            "position requires a map with 'pos'",
                        ))
                    }
                };
                self.seek(id, pos)?;
            }
            HandleOperation::Cursor => {
                let handle = self.handle_mut(id)?;
                let content = handle.encoding.decode(value)?;
                handle.file.write_all(&content)?;
            }
            HandleOperation::AtOffset { offset } => {
                let handle = self.handle_mut(id)?;
                let content = handle.encoding.decode(value)?;
                handle.file.seek(SeekFrom::Start(offset))?;
                handle.file.write_all(&content)?;
            }
            HandleOperation::ReadAtLen { .. } | HandleOperation::Meta => {
                return Err(Error::permission_denied(format!("{path} is read-only")))
            }
        }
        Ok(path.clone())
    }

    /// Move handle `id`'s cursor to `pos`.
    pub(super) fn seek(&mut self, id: u64, pos: u64) -> Result<(), Error> {
        self.handle_mut(id)?.file.seek(SeekFrom::Start(pos))?;
        Ok(())
    }
}
