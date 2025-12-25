//! Filesystem operations store.
//!
//! Provides filesystem operations through StructFS paths.
//!
//! ## Paths
//!
//! - `fs/stat` - Write `{"path": "/some/path"}` to get file info
//! - `fs/readdir` - Write `{"path": "/some/dir"}` to list directory
//! - `fs/mkdir` - Write `{"path": "/some/dir"}` to create directory
//! - `fs/rmdir` - Write `{"path": "/some/dir"}` to remove directory
//! - `fs/unlink` - Write `{"path": "/some/file"}` to delete file
//! - `fs/rename` - Write `{"from": "/a", "to": "/b"}` to rename
//! - `fs/exists` - Write `{"path": "/some/path"}` to check existence

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use structfs_store::{Error, Path, Reader, Writer};

/// Store for filesystem operations.
pub struct FsStore;

impl FsStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.is_empty() {
            return Ok(Some(json!({
                "stat": "Write {\"path\": \"...\"} to get file info",
                "readdir": "Write {\"path\": \"...\"} to list directory",
                "mkdir": "Write {\"path\": \"...\", \"recursive\": bool} to create directory",
                "rmdir": "Write {\"path\": \"...\"} to remove directory",
                "unlink": "Write {\"path\": \"...\"} to delete file",
                "rename": "Write {\"from\": \"...\", \"to\": \"...\"} to rename",
                "exists": "Write {\"path\": \"...\"} to check existence"
            })));
        }

        // All fs operations are action-based (write to perform)
        Ok(None)
    }
}

impl Default for FsStore {
    fn default() -> Self {
        Self::new()
    }
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

impl Reader for FsStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
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
        if path.components.len() != 1 {
            return Err(Error::ImplementationFailure {
                message: "Invalid fs path".to_string(),
            });
        }

        let value = serde_json::to_value(data).map_err(|err| Error::RecordSerialization {
            message: format!("Failed to serialize fs request: {}", err),
        })?;

        match path.components[0].as_str() {
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

                // TODO: Return the stat result somehow
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

                // TODO: Return entries somehow
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

                fs::rename(&request.from, &request.to).map_err(|e| {
                    Error::ImplementationFailure {
                        message: format!("rename failed: {}", e),
                    }
                })?;

                Ok(path.clone())
            }
            "exists" => {
                let request: PathRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid exists request: {}", e),
                    })?;

                let _exists = std::path::Path::new(&request.path).exists();
                // TODO: Return result
                Ok(path.clone())
            }
            _ => Err(Error::ImplementationFailure {
                message: format!("Unknown fs operation: {}", path.components[0]),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_mkdir_and_rmdir() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let new_dir = dir.path().join("test_dir");
        let path_str = new_dir.to_string_lossy().to_string();

        // Create directory
        store
            .write(&Path::parse("mkdir").unwrap(), json!({"path": path_str}))
            .unwrap();

        assert!(new_dir.exists());

        // Remove directory
        store
            .write(&Path::parse("rmdir").unwrap(), json!({"path": path_str}))
            .unwrap();

        assert!(!new_dir.exists());
    }

    #[test]
    fn test_unlink() {
        let mut store = FsStore::new();
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_file.txt");

        // Create a file
        File::create(&file_path).unwrap();
        assert!(file_path.exists());

        // Delete it
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

        // Create a file
        File::create(&old_path).unwrap();
        assert!(old_path.exists());

        // Rename it
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
