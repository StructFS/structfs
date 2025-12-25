//! Documentation store for sys primitives.
//!
//! Provides help content at the `docs` path within the sys store.
//! - `read docs` - Overview of sys store
//! - `read docs/env` - Environment variables help
//! - `read docs/time` - Time operations help
//! - `read docs/random` - Random generation help
//! - `read docs/proc` - Process info help
//! - `read docs/fs` - Filesystem operations help

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};

use structfs_store::{Error as StoreError, Path, Reader, Writer};

/// Documentation store for sys primitives.
pub struct DocsStore;

impl DocsStore {
    pub fn new() -> Self {
        Self
    }

    fn get_docs(&self, path: &Path) -> Option<JsonValue> {
        if path.is_empty() {
            return Some(self.root_docs());
        }

        match path.components[0].as_str() {
            "env" => Some(self.env_docs(&path.components[1..])),
            "time" => Some(self.time_docs(&path.components[1..])),
            "random" => Some(self.random_docs(&path.components[1..])),
            "proc" => Some(self.proc_docs(&path.components[1..])),
            "fs" => Some(self.fs_docs(&path.components[1..])),
            _ => None,
        }
    }

    fn root_docs(&self) -> JsonValue {
        json!({
            "title": "System Primitives",
            "description": "OS primitives exposed through StructFS paths.",
            "subsystems": {
                "env": "Environment variables - read, write, list",
                "time": "Clocks and sleep - current time, monotonic, delays",
                "random": "Random generation - integers, UUIDs, bytes",
                "proc": "Process info - PID, CWD, args, environment",
                "fs": "Filesystem - open, read, write, stat, mkdir, etc."
            },
            "examples": [
                "read env/HOME",
                "read time/now",
                "read random/uuid",
                "read proc/self/pid",
                "write fs/open {\"path\": \"/tmp/test\", \"mode\": \"write\"}"
            ],
            "see_also": ["docs/env", "docs/time", "docs/random", "docs/proc", "docs/fs"]
        })
    }

    fn env_docs(&self, subpath: &[String]) -> JsonValue {
        if subpath.is_empty() {
            json!({
                "title": "Environment Variables",
                "description": "Read and write process environment variables.",
                "operations": {
                    "read env": "List all environment variables as an object",
                    "read env/<NAME>": "Read a specific variable (returns string or null)",
                    "write env/<NAME> \"value\"": "Set an environment variable",
                    "write env/<NAME> null": "Unset an environment variable"
                },
                "examples": [
                    {"command": "read env/HOME", "result": "\"/Users/alice\""},
                    {"command": "read env/PATH", "result": "\"/usr/bin:/bin\""},
                    {"command": "write env/MY_VAR \"hello\"", "result": "Sets MY_VAR"},
                    {"command": "write env/MY_VAR null", "result": "Unsets MY_VAR"}
                ],
                "notes": [
                    "Variable names are case-sensitive on Unix, case-insensitive on Windows",
                    "Changes affect the current process only"
                ]
            })
        } else {
            json!({
                "description": format!("Environment variable: {}", subpath.join("/")),
                "usage": {
                    "read": "Returns the variable value as a string, or null if unset",
                    "write": "Set to a string value, or null to unset"
                }
            })
        }
    }

    fn time_docs(&self, subpath: &[String]) -> JsonValue {
        if subpath.is_empty() {
            json!({
                "title": "Time Operations",
                "description": "Clocks, timestamps, and delays.",
                "paths": {
                    "now": "Current time as ISO 8601 string",
                    "now_unix": "Unix timestamp in seconds (integer)",
                    "now_unix_ms": "Unix timestamp in milliseconds (integer)",
                    "monotonic": "Monotonic clock in nanoseconds (for measuring durations)",
                    "sleep": "Write {\"ms\": N} to sleep for N milliseconds"
                },
                "examples": [
                    {"command": "read time/now", "result": "\"2024-01-15T10:30:00Z\""},
                    {"command": "read time/now_unix", "result": "1705315800"},
                    {"command": "read time/monotonic", "result": "123456789000"},
                    {"command": "write time/sleep {\"ms\": 100}", "result": "Sleeps for 100ms"}
                ]
            })
        } else {
            match subpath[0].as_str() {
                "now" => json!({
                    "path": "time/now",
                    "description": "Current wall-clock time in ISO 8601 format",
                    "returns": "String like \"2024-01-15T10:30:00.123Z\""
                }),
                "now_unix" => json!({
                    "path": "time/now_unix",
                    "description": "Unix timestamp (seconds since 1970-01-01 UTC)",
                    "returns": "Integer"
                }),
                "now_unix_ms" => json!({
                    "path": "time/now_unix_ms",
                    "description": "Unix timestamp in milliseconds",
                    "returns": "Integer"
                }),
                "monotonic" => json!({
                    "path": "time/monotonic",
                    "description": "Monotonic clock value in nanoseconds. Use for measuring elapsed time.",
                    "returns": "Integer (nanoseconds since arbitrary epoch)"
                }),
                "sleep" => json!({
                    "path": "time/sleep",
                    "description": "Block execution for a duration",
                    "write_format": "{\"ms\": <milliseconds>}",
                    "example": "write time/sleep {\"ms\": 500}"
                }),
                _ => json!({"error": format!("Unknown time path: {}", subpath.join("/"))})
            }
        }
    }

    fn random_docs(&self, subpath: &[String]) -> JsonValue {
        if subpath.is_empty() {
            json!({
                "title": "Random Number Generation",
                "description": "Cryptographically secure random values.",
                "paths": {
                    "u64": "Random 64-bit unsigned integer",
                    "uuid": "Random UUID v4",
                    "bytes": "Write {\"count\": N} to get N random bytes (base64)"
                },
                "examples": [
                    {"command": "read random/u64", "result": "12345678901234567890"},
                    {"command": "read random/uuid", "result": "\"550e8400-e29b-41d4-a716-446655440000\""},
                    {"command": "write random/bytes {\"count\": 16}", "result": "\"base64-encoded-bytes\""}
                ]
            })
        } else {
            match subpath[0].as_str() {
                "u64" => json!({
                    "path": "random/u64",
                    "description": "Generate a random 64-bit unsigned integer",
                    "returns": "Integer in range [0, 2^64)"
                }),
                "uuid" => json!({
                    "path": "random/uuid",
                    "description": "Generate a random UUID version 4",
                    "returns": "String in format \"xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx\""
                }),
                "bytes" => json!({
                    "path": "random/bytes",
                    "description": "Generate random bytes",
                    "write_format": "{\"count\": <number_of_bytes>}",
                    "returns": "Base64-encoded string of random bytes"
                }),
                _ => json!({"error": format!("Unknown random path: {}", subpath.join("/"))})
            }
        }
    }

    fn proc_docs(&self, subpath: &[String]) -> JsonValue {
        if subpath.is_empty() {
            json!({
                "title": "Process Information",
                "description": "Information about the current process.",
                "paths": {
                    "self/pid": "Current process ID",
                    "self/cwd": "Current working directory",
                    "self/args": "Command-line arguments as array",
                    "self/exe": "Path to the executable",
                    "self/env": "All environment variables as object"
                },
                "examples": [
                    {"command": "read proc/self/pid", "result": "12345"},
                    {"command": "read proc/self/cwd", "result": "\"/home/user/project\""},
                    {"command": "read proc/self/args", "result": "[\"structfs\", \"--help\"]"}
                ]
            })
        } else if subpath[0] == "self" {
            if subpath.len() == 1 {
                json!({
                    "path": "proc/self",
                    "description": "Current process information",
                    "children": ["pid", "cwd", "args", "exe", "env"]
                })
            } else {
                match subpath[1].as_str() {
                    "pid" => json!({
                        "path": "proc/self/pid",
                        "description": "Process ID of the current process",
                        "returns": "Integer"
                    }),
                    "cwd" => json!({
                        "path": "proc/self/cwd",
                        "description": "Current working directory",
                        "returns": "String (absolute path)"
                    }),
                    "args" => json!({
                        "path": "proc/self/args",
                        "description": "Command-line arguments",
                        "returns": "Array of strings"
                    }),
                    "exe" => json!({
                        "path": "proc/self/exe",
                        "description": "Path to the current executable",
                        "returns": "String (absolute path)"
                    }),
                    "env" => json!({
                        "path": "proc/self/env",
                        "description": "All environment variables",
                        "returns": "Object mapping names to values"
                    }),
                    _ => json!({"error": format!("Unknown proc/self path: {}", subpath[1..].join("/"))})
                }
            }
        } else {
            json!({"error": format!("Unknown proc path: {}", subpath.join("/"))})
        }
    }

    fn fs_docs(&self, subpath: &[String]) -> JsonValue {
        if subpath.is_empty() {
            json!({
                "title": "Filesystem Operations",
                "description": "File and directory operations with handle-based I/O.",
                "operations": {
                    "open": "Open a file, returns a handle path",
                    "handles/<id>": "Read/write file content through handle",
                    "handles/<id>/seek": "Seek within file",
                    "handles/<id>/close": "Close the handle",
                    "stat": "Get file/directory metadata",
                    "mkdir": "Create a directory",
                    "rmdir": "Remove an empty directory",
                    "unlink": "Delete a file",
                    "rename": "Rename/move a file or directory",
                    "readdir": "List directory contents"
                },
                "open_modes": {
                    "read": "Open for reading (file must exist)",
                    "write": "Open for writing (truncates if exists, creates if not)",
                    "append": "Open for appending",
                    "readwrite": "Open for both reading and writing",
                    "create_new": "Create new file (fails if exists)"
                },
                "encodings": {
                    "base64": "Default - binary-safe, all content base64 encoded",
                    "utf8": "UTF-8 text (errors on invalid sequences)",
                    "latin1": "ISO-8859-1 (any byte sequence valid)",
                    "ascii": "ASCII only (errors on bytes > 127)"
                },
                "examples": [
                    {
                        "description": "Open, write, and close a file",
                        "commands": [
                            "@h write fs/open {\"path\": \"/tmp/test.txt\", \"mode\": \"write\", \"encoding\": \"utf8\"}",
                            "write *@h \"Hello, World!\"",
                            "write *@h/close null"
                        ]
                    },
                    {
                        "description": "Read a file",
                        "commands": [
                            "@h write fs/open {\"path\": \"/tmp/test.txt\", \"mode\": \"read\", \"encoding\": \"utf8\"}",
                            "read *@h",
                            "write *@h/close null"
                        ]
                    }
                ],
                "see_also": ["docs/fs/open", "docs/fs/stat", "docs/fs/handles"]
            })
        } else {
            match subpath[0].as_str() {
                "open" => json!({
                    "path": "fs/open",
                    "description": "Open a file and get a handle for I/O",
                    "write_format": {
                        "path": "Required - absolute path to the file",
                        "mode": "Optional - read|write|append|readwrite|create_new (default: read)",
                        "encoding": "Optional - base64|utf8|latin1|ascii (default: base64)"
                    },
                    "returns": "Handle path like \"handles/1\"",
                    "example": "write fs/open {\"path\": \"/tmp/file.txt\", \"mode\": \"readwrite\", \"encoding\": \"utf8\"}"
                }),
                "handles" => json!({
                    "path": "fs/handles",
                    "description": "Open file handles",
                    "operations": {
                        "read handles/<id>": "Read file content (encoded per handle's encoding setting)",
                        "write handles/<id> <content>": "Write content to file",
                        "write handles/<id>/seek {\"pos\": N}": "Seek to absolute position",
                        "write handles/<id>/seek {\"offset\": N, \"whence\": \"current\"}": "Relative seek",
                        "write handles/<id>/close null": "Close the handle"
                    },
                    "seek_whence": ["start", "current", "end"]
                }),
                "stat" => json!({
                    "path": "fs/stat",
                    "description": "Get metadata about a file or directory",
                    "write_format": "{\"path\": \"/path/to/file\"}",
                    "returns": {
                        "path": "Absolute path",
                        "exists": "Boolean",
                        "is_file": "Boolean",
                        "is_dir": "Boolean",
                        "size": "Size in bytes (files only)",
                        "modified": "Last modification time (ISO 8601)"
                    }
                }),
                "mkdir" => json!({
                    "path": "fs/mkdir",
                    "description": "Create a directory",
                    "write_format": "{\"path\": \"/path/to/new/dir\"}",
                    "notes": ["Parent directories must exist", "Fails if directory already exists"]
                }),
                "rmdir" => json!({
                    "path": "fs/rmdir",
                    "description": "Remove an empty directory",
                    "write_format": "{\"path\": \"/path/to/dir\"}",
                    "notes": ["Directory must be empty"]
                }),
                "unlink" => json!({
                    "path": "fs/unlink",
                    "description": "Delete a file",
                    "write_format": "{\"path\": \"/path/to/file\"}",
                    "notes": ["Cannot delete directories (use rmdir)"]
                }),
                "rename" => json!({
                    "path": "fs/rename",
                    "description": "Rename or move a file or directory",
                    "write_format": "{\"from\": \"/old/path\", \"to\": \"/new/path\"}"
                }),
                "readdir" => json!({
                    "path": "fs/readdir",
                    "description": "List directory contents",
                    "write_format": "{\"path\": \"/path/to/dir\"}",
                    "returns": "Array of entry names (strings)"
                }),
                _ => json!({"error": format!("Unknown fs path: {}", subpath.join("/"))})
            }
        }
    }
}

impl Default for DocsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for DocsStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        match self.get_docs(from) {
            Some(docs) => Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(docs)))),
            None => Ok(None),
        }
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        match self.get_docs(from) {
            Some(docs) => {
                let record = serde_json::from_value(docs).map_err(|e| {
                    StoreError::RecordDeserialization {
                        message: e.to_string(),
                    }
                })?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }
}

impl Writer for DocsStore {
    fn write<RecordType: Serialize>(
        &mut self,
        _destination: &Path,
        _data: RecordType,
    ) -> Result<Path, StoreError> {
        Err(StoreError::Raw {
            message: "Documentation is read-only".to_string(),
        })
    }
}
