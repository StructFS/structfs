//! Path-addressed filesystem actions (`open`, `stat`, `readdir`, `mkdir`,
//! `rmdir`, `unlink`, `rename`), root confinement, and the `results` table
//! that carries `stat`/`readdir` answers back to the caller.

use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::ErrorKind;
use std::path::{Component, PathBuf};

use collection_literals::btree;
use structfs_core_store::{path, Error, Path, Reference, Value};

use super::encoding::{optional_str, request_map, required_str, ContentEncoding, OpenMode};
use super::handles::{handle_path, FileHandle};
use super::FsStore;

/// Whether confinement resolves a final symlink component.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Final {
    /// Operate on what the path points at (open, stat, readdir, mkdir).
    Follow,
    /// Operate on the directory entry itself (unlink, rmdir, rename).
    Entry,
}

/// The data path of result `id`.
fn result_path(id: u64) -> Path {
    let mut path = path!("results");
    path.push(id);
    path
}

fn kind_of(metadata: &Metadata) -> &'static str {
    let kind = metadata.file_type();
    if kind.is_symlink() {
        "symlink"
    } else if kind.is_dir() {
        "dir"
    } else if kind.is_file() {
        "file"
    } else {
        "other"
    }
}

fn stat_value(metadata: &Metadata) -> Value {
    let modified = metadata
        .modified()
        .ok()
        .map(|time| Value::String(chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()))
        .unwrap_or(Value::Null);
    Value::Map(btree! {
        "size".into() => Value::Unsigned(metadata.len()),
        "kind".into() => Value::String(kind_of(metadata).into()),
        "modified".into() => modified,
        "readonly".into() => Value::Bool(metadata.permissions().readonly()),
    })
}

impl FsStore {
    /// Resolve a caller-supplied OS path.
    ///
    /// Unrooted stores use the path as given. Rooted stores join relative
    /// paths onto the root, resolve symlinks in every existing component
    /// (and in the final one for [`Final::Follow`]), and refuse anything
    /// that lands outside the root with `PermissionDenied`. Components that
    /// do not exist yet may not contain `..`.
    pub(super) fn confine(&self, raw: &str, last: Final) -> Result<PathBuf, Error> {
        let Some(root) = &self.root else {
            return Ok(PathBuf::from(raw));
        };
        let candidate = root.join(raw);

        // Walk up to the deepest existing entry, remembering the missing tail.
        let mut existing = candidate.clone();
        let mut tail = Vec::new();
        loop {
            match fs::symlink_metadata(&existing) {
                Ok(_) => break,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    let name = match existing.components().next_back() {
                        Some(Component::Normal(name)) => name.to_owned(),
                        _ => {
                            return Err(Error::invalid_argument(format!(
                                "cannot resolve '{raw}' inside the fs root"
                            )))
                        }
                    };
                    tail.push(name);
                    existing = existing
                        .parent()
                        .ok_or_else(|| Error::invalid_argument(format!("invalid path '{raw}'")))?
                        .to_path_buf();
                }
                Err(e) => return Err(e.into()),
            }
        }

        let mut resolved = if tail.is_empty() && last == Final::Entry {
            // Resolve the parent only; keep the final entry itself.
            match (existing.parent(), existing.components().next_back()) {
                (Some(parent), Some(Component::Normal(name))) => {
                    fs::canonicalize(parent)?.join(name)
                }
                _ => fs::canonicalize(&existing)?,
            }
        } else {
            fs::canonicalize(&existing).map_err(|e| {
                if e.kind() == ErrorKind::NotFound {
                    // A dangling symlink: its target is unknown, so it may
                    // point anywhere.
                    Error::permission_denied(format!(
                        "'{raw}' is a dangling symlink; refusing to follow it"
                    ))
                } else {
                    e.into()
                }
            })?
        };
        for name in tail.into_iter().rev() {
            resolved.push(name);
        }

        if last == Final::Entry && resolved == *root {
            // rmdir/unlink/rename of the root itself would remove or move
            // the confinement boundary.
            Err(Error::permission_denied(format!(
                "'{raw}' is the fs root itself"
            )))
        } else if resolved.starts_with(root) {
            Ok(resolved)
        } else {
            Err(Error::permission_denied(format!(
                "'{raw}' resolves outside the fs root"
            )))
        }
    }

    fn os_path(
        &self,
        map: &BTreeMap<String, Value>,
        field: &str,
        action: &str,
        last: Final,
    ) -> Result<PathBuf, Error> {
        self.confine(required_str(map, field, action)?, last)
    }

    /// `write open {"path", "mode"?, "encoding"?}` → `handles/{id}`.
    pub(super) fn open(&mut self, value: &Value) -> Result<Path, Error> {
        let map = request_map(value, "open")?;
        if self.handles.len() >= self.max_handles {
            return Err(Error::resource_limit(format!(
                "at most {} fs handles may be open; close one first",
                self.max_handles
            )));
        }
        let mode = optional_str(map, "mode", "open")?
            .map(OpenMode::parse)
            .transpose()?
            .unwrap_or_default();
        let encoding = optional_str(map, "encoding", "open")?
            .map(ContentEncoding::parse)
            .transpose()?
            .unwrap_or_default();
        let os_path = self.os_path(map, "path", "open", Final::Follow)?;

        let file = match mode {
            OpenMode::Read => File::open(&os_path),
            OpenMode::Write => File::create(&os_path),
            OpenMode::Append => OpenOptions::new().append(true).create(true).open(&os_path),
            OpenMode::ReadWrite => OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&os_path),
            OpenMode::CreateNew => OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&os_path),
        }?;

        let id = self.next_id();
        self.handles.insert(
            id,
            FileHandle {
                file,
                path: os_path.to_string_lossy().into_owned(),
                mode,
                encoding,
            },
        );
        Ok(handle_path(id))
    }

    /// Store an answer in the bounded `results` table; the oldest answer is
    /// evicted once `max_results` are retained.
    fn store_result(&mut self, value: Value) -> Path {
        while self.results.len() >= self.max_results {
            self.results.pop_first();
        }
        let id = self.next_id();
        self.results.insert(id, value);
        result_path(id)
    }

    /// `read results`: retained answers, in id order.
    pub(super) fn results_listing(&self) -> Value {
        let items = self
            .results
            .keys()
            .map(|id| Reference::with_type(format!("results/{id}"), "result").to_value())
            .collect();
        Value::Map(btree! { "items".into() => Value::Array(items) })
    }

    /// `write results/{id} null` discards an answer.
    pub(super) fn write_result(&mut self, to: &Path, value: &Value) -> Result<Path, Error> {
        let id = match to.len() {
            2 => to[1].parse::<u64>().ok(),
            _ => None,
        }
        .ok_or_else(|| Error::invalid_argument(format!("invalid result path: {to}")))?;
        if *value != Value::Null {
            return Err(Error::permission_denied(
                "results are read-only; write null to discard one",
            ));
        }
        self.results
            .remove(&id)
            .ok_or_else(|| Error::not_found(result_path(id)))?;
        Ok(to.clone())
    }

    /// Run a path action other than `open`.
    pub(super) fn action(&mut self, action: &str, value: &Value) -> Result<Path, Error> {
        let map = request_map(value, action)?;
        match action {
            "stat" => {
                let os_path = self.os_path(map, "path", action, Final::Follow)?;
                let stat = stat_value(&fs::metadata(os_path)?);
                Ok(self.store_result(stat))
            }
            "readdir" => {
                let os_path = self.os_path(map, "path", action, Final::Follow)?;
                let mut entries = Vec::new();
                for entry in fs::read_dir(os_path)? {
                    let entry = entry?;
                    let kind = kind_of(&entry.metadata()?);
                    entries.push((entry.file_name().to_string_lossy().into_owned(), kind));
                }
                entries.sort();
                let listing = entries
                    .into_iter()
                    .map(|(name, kind)| {
                        Value::Map(btree! {
                            "name".into() => Value::String(name),
                            "kind".into() => Value::String(kind.into()),
                        })
                    })
                    .collect();
                Ok(self.store_result(Value::Array(listing)))
            }
            "mkdir" => {
                let os_path = self.os_path(map, "path", action, Final::Follow)?;
                match map.get("recursive") {
                    None | Some(Value::Bool(false)) => fs::create_dir(os_path)?,
                    Some(Value::Bool(true)) => fs::create_dir_all(os_path)?,
                    Some(_) => {
                        return Err(Error::invalid_argument(
                            "mkdir: 'recursive' must be a boolean",
                        ))
                    }
                }
                Ok(path!("mkdir"))
            }
            "rmdir" => {
                fs::remove_dir(self.os_path(map, "path", action, Final::Entry)?)?;
                Ok(path!("rmdir"))
            }
            "unlink" => {
                fs::remove_file(self.os_path(map, "path", action, Final::Entry)?)?;
                Ok(path!("unlink"))
            }
            "rename" => {
                let from = self.os_path(map, "from", action, Final::Entry)?;
                let to = self.os_path(map, "to", action, Final::Entry)?;
                fs::rename(from, to)?;
                Ok(path!("rename"))
            }
            other => Err(Error::invalid_argument(format!(
                "unknown fs operation: {other}"
            ))),
        }
    }
}
