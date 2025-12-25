//! Help store that provides documentation via read operations.
//!
//! Mount at `/ctx/help` to provide in-REPL documentation:
//! - `read /ctx/help` - Overview and topic list
//! - `read /ctx/help/commands` - Available commands
//! - `read /ctx/help/mounts` - Mount system documentation
//! - `read /ctx/help/http` - HTTP broker usage
//!
//! ## Docs Protocol
//!
//! The HelpStore supports the docs protocol: stores that provide documentation
//! at a `docs` path can have that docs path mounted into the HelpStore. This
//! enables reading `help/sys` to return the sys store's own documentation.
//!
//! ```text
//! # Register a store's docs:
//! help_store.mount_docs("sys", sys_docs_store);
//!
//! # Now reading help/sys returns sys's documentation
//! read /ctx/help/sys  -> reads from mounted sys docs
//! ```

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};

use structfs_store::{Error as StoreError, OverlayStore, Path, Reader, Store, Writer};

/// A store that returns help documentation on read.
///
/// The HelpStore combines:
/// 1. Built-in help topics (commands, mounts, http, etc.)
/// 2. Mounted documentation from other stores (via the docs protocol)
///
/// When reading a path, it first checks if there's a mounted docs store
/// for that path prefix, then falls back to built-in help.
pub struct HelpStore<'a> {
    /// Mounted docs from other stores, keyed by topic name
    mounted_docs: OverlayStore<'a>,
    /// Track which topics have mounted docs
    mounted_topics: Vec<String>,
}

impl<'a> HelpStore<'a> {
    pub fn new() -> Self {
        Self {
            mounted_docs: OverlayStore::default(),
            mounted_topics: Vec::new(),
        }
    }

    /// Mount a store's documentation at a topic path.
    ///
    /// For example, `mount_docs("sys", sys_docs_store)` makes the sys store's
    /// documentation available at `help/sys`.
    pub fn mount_docs<S: Store + Send + Sync + 'a>(&mut self, topic: &str, docs_store: S) {
        let path = Path::parse(topic).expect("valid topic path");
        self.mounted_docs
            .add_layer(path, docs_store)
            .expect("mounting docs should succeed");
        self.mounted_topics.push(topic.to_string());
    }

    /// Check if a topic has mounted documentation
    fn has_mounted_docs(&self, topic: &str) -> bool {
        self.mounted_topics.iter().any(|t| t == topic)
    }

    /// Try to read from mounted docs for a topic
    fn try_mounted_docs(&mut self, path: &Path) -> Option<JsonValue> {
        if path.is_empty() {
            return None;
        }

        let topic = &path.components[0];
        if !self.has_mounted_docs(topic) {
            return None;
        }

        // Try to read from mounted docs
        match self.mounted_docs.read_owned::<JsonValue>(path) {
            Ok(Some(value)) => Some(value),
            _ => None,
        }
    }

    fn get_help(&mut self, path: &Path) -> JsonValue {
        if path.is_empty() {
            return self.root_help();
        }

        // First, try mounted docs for the topic
        if let Some(docs) = self.try_mounted_docs(path) {
            return docs;
        }

        // Check for system paths (interpret from root)
        let full_path = path.components.join("/");
        match full_path.as_str() {
            // Context mounts
            "ctx" => self.ctx_help(),
            "ctx/http" => self.http_help(),
            "ctx/help" => self.root_help(),
            // Mount system
            "ctx/mounts" => self.mounts_help(),
            // Context sys - if sys docs not mounted, redirect
            "ctx/sys" => {
                if self.has_mounted_docs("sys") {
                    self.try_mounted_docs(&Path::parse("sys").unwrap())
                        .unwrap_or_else(|| self.store_docs_redirect("ctx/sys"))
                } else {
                    self.store_docs_redirect("ctx/sys")
                }
            }
            // Topic-based help (single component)
            _ => {
                // Check for ctx/sys/* paths - redirect to store docs or use mounted
                if let Some(subpath) = full_path.strip_prefix("ctx/sys/") {
                    if self.has_mounted_docs("sys") {
                        let docs_path = Path::parse(&format!("sys/{}", subpath)).unwrap();
                        if let Some(docs) = self.try_mounted_docs(&docs_path) {
                            return docs;
                        }
                    }
                    return self.store_docs_redirect_with_path("ctx/sys", subpath);
                }

                match path.components[0].as_str() {
                    "commands" => self.commands_help(),
                    "mounts" => self.mounts_help(),
                    "http" => self.http_help(),
                    "paths" => self.paths_help(),
                    "examples" => self.examples_help(),
                    "stores" => self.stores_help(),
                    "registers" => self.registers_help(),
                    _ => self.suggest_help(&full_path),
                }
            }
        }
    }

    /// Redirect to a store's own documentation
    fn store_docs_redirect(&self, store_path: &str) -> JsonValue {
        json!({
            "message": format!("This store provides its own documentation."),
            "read_docs": format!("read /{}/docs", store_path),
            "hint": "Stores that support the docs protocol expose documentation at their 'docs' path."
        })
    }

    /// Redirect to a store's own documentation with a subpath
    fn store_docs_redirect_with_path(&self, store_path: &str, subpath: &str) -> JsonValue {
        json!({
            "message": format!("This store provides its own documentation."),
            "read_docs": format!("read /{}/docs/{}", store_path, subpath),
            "hint": "Stores that support the docs protocol expose documentation at their 'docs' path."
        })
    }

    /// Suggest relevant help topics based on an unknown path
    fn suggest_help(&self, query: &str) -> JsonValue {
        let query_lower = query.to_lowercase();
        let mut suggestions = Vec::new();

        // Keywords mapped to relevant topics
        let keyword_topics: &[(&[&str], &str, &str)] = &[
            // (keywords, topic, description)
            (
                &[
                    "read", "write", "cd", "pwd", "exit", "quit", "command", "cmd",
                ],
                "commands",
                "REPL commands",
            ),
            (
                &[
                    "mount", "unmount", "attach", "store", "memory", "local", "remote",
                ],
                "mounts",
                "Mount system",
            ),
            (
                &[
                    "http", "request", "get", "post", "put", "delete", "api", "url", "fetch",
                    "web", "rest",
                ],
                "http",
                "HTTP requests",
            ),
            (
                &[
                    "path",
                    "directory",
                    "dir",
                    "folder",
                    "navigate",
                    "cd",
                    "pwd",
                    "/",
                    "relative",
                    "absolute",
                ],
                "paths",
                "Path syntax",
            ),
            (
                &["example", "tutorial", "howto", "how-to", "demo", "sample"],
                "examples",
                "Usage examples",
            ),
            (
                &[
                    "store", "memory", "local", "disk", "persist", "storage", "backend",
                ],
                "stores",
                "Store types",
            ),
            (
                &[
                    "register",
                    "@",
                    "variable",
                    "capture",
                    "output",
                    "save",
                    "dereference",
                    "*@",
                ],
                "registers",
                "Registers",
            ),
            (
                &[
                    "sys",
                    "env",
                    "environment",
                    "time",
                    "clock",
                    "random",
                    "proc",
                    "process",
                    "fs",
                    "file",
                    "filesystem",
                    "open",
                    "handle",
                ],
                "sys",
                "System primitives",
            ),
            (
                &["ctx", "context", "built-in", "builtin", "default"],
                "ctx",
                "Context directory",
            ),
        ];

        // Check for matching keywords
        for (keywords, topic, description) in keyword_topics {
            for keyword in *keywords {
                if query_lower.contains(keyword) {
                    let suggestion = format!("{} - {}", topic, description);
                    if !suggestions.contains(&suggestion) {
                        suggestions.push(suggestion);
                    }
                    break;
                }
            }
        }

        // Check for partial matches in topic names
        let topics = [
            "commands",
            "mounts",
            "http",
            "paths",
            "examples",
            "stores",
            "registers",
            "sys",
        ];
        for topic in topics {
            if topic.contains(&query_lower) || query_lower.contains(topic) {
                let suggestion = topic.to_string();
                if !suggestions.iter().any(|s| s.starts_with(topic)) {
                    suggestions.push(suggestion);
                }
            }
        }

        // If no suggestions, provide general help
        if suggestions.is_empty() {
            json!({
                "error": format!("No help found for: '{}'", query),
                "hint": "Try one of the available topics below",
                "available_topics": {
                    "commands": "REPL commands (read, write, cd, etc.)",
                    "mounts": "Mounting and managing stores",
                    "http": "Making HTTP requests",
                    "paths": "Path syntax and navigation",
                    "registers": "Capturing and using command output (@name, *@name)",
                    "examples": "Usage examples",
                    "stores": "Available store types",
                    "sys": "System primitives (env, time, fs, proc, random)"
                },
                "system_paths": ["ctx", "ctx/http", "ctx/help", "ctx/mounts", "ctx/sys"],
                "tip": "You can also get help for system paths, e.g., 'read /ctx/help/ctx/sys'"
            })
        } else {
            json!({
                "message": format!("No exact match for '{}', but these topics might help:", query),
                "suggestions": suggestions,
                "try": suggestions.iter().map(|s| {
                    let topic = s.split(" - ").next().unwrap_or(s);
                    format!("read /ctx/help/{}", topic)
                }).collect::<Vec<_>>(),
                "all_topics": ["commands", "mounts", "http", "paths", "examples", "stores", "registers", "sys"]
            })
        }
    }

    fn ctx_help(&self) -> JsonValue {
        json!({
            "title": "Context Directory (/ctx)",
            "description": "The /ctx directory contains built-in system stores.",
            "mounts": {
                "/ctx/http": "Async HTTP broker - requests execute in background",
                "/ctx/http_sync": "Sync HTTP broker - blocks on read until complete",
                "/ctx/help": "This help system",
                "/ctx/mounts": "Mount management - create and manage store mounts",
                "/ctx/sys": "System primitives (env, time, proc, fs, random)"
            },
            "usage": [
                "read /ctx/help          - Get help",
                "read /ctx/help/http     - Help on HTTP broker",
                "write /ctx/http <req>   - Queue an HTTP request (async)",
                "read /ctx/http/outstanding/0         - Check status",
                "read /ctx/http/outstanding/0/response - Get response when complete",
                "read /ctx/sys/env/HOME  - Read environment variable",
                "read /ctx/sys/time/now  - Get current time"
            ]
        })
    }

    fn root_help(&self) -> JsonValue {
        json!({
            "title": "StructFS REPL Help",
            "description": "StructFS provides a uniform interface for accessing data through read/write operations on paths.",
            "topics": {
                "commands": "Available REPL commands",
                "mounts": "Mounting and managing stores",
                "http": "Making HTTP requests",
                "paths": "Path syntax and navigation",
                "registers": "Registers for storing command output",
                "examples": "Usage examples",
                "stores": "Available store types",
                "sys": "System primitives (env, time, proc, fs, random)"
            },
            "quick_start": [
                "read /ctx/mounts          - List current mounts",
                "write /ctx/mounts/data {\"type\": \"memory\"}  - Create a memory store at /data",
                "write /data/hello {\"message\": \"world\"}  - Write data",
                "read /data/hello       - Read data back"
            ]
        })
    }

    fn commands_help(&self) -> JsonValue {
        json!({
            "title": "REPL Commands",
            "commands": {
                "read <path>": "Read data from a path (aliases: get, r)",
                "write <path> <json>": "Write JSON data to a path (aliases: set, w)",
                "cd <path>": "Change current directory",
                "pwd": "Print current directory",
                "mounts": "List all current mounts (shortcut for read /ctx/mounts)",
                "help": "Show help message",
                "exit": "Exit the REPL (aliases: quit)"
            },
            "examples": [
                "read /ctx/help",
                "write /ctx/mounts/test {\"type\": \"memory\"}",
                "cd /test",
                "write foo {\"bar\": 123}",
                "read foo"
            ]
        })
    }

    fn mounts_help(&self) -> JsonValue {
        json!({
            "title": "Mount System",
            "description": "Mounts attach stores to paths in the filesystem tree. Manage mounts through /ctx/mounts.",
            "operations": {
                "read /ctx/mounts": "List all mounts",
                "read /ctx/mounts/<name>": "Get config for a specific mount",
                "write /ctx/mounts/<name> <config>": "Create or update a mount",
                "write /ctx/mounts/<name> null": "Unmount a store"
            },
            "mount_configs": {
                "memory": "{\"type\": \"memory\"}",
                "local": "{\"type\": \"local\", \"path\": \"/path/to/dir\"}",
                "http": "{\"type\": \"http\", \"url\": \"https://api.example.com\"}",
                "httpbroker": "{\"type\": \"httpbroker\"} (sync)",
                "asynchttpbroker": "{\"type\": \"asynchttpbroker\"} (async, background threads)",
                "structfs": "{\"type\": \"structfs\", \"url\": \"https://structfs.example.com\"}"
            },
            "examples": [
                "write /ctx/mounts/data {\"type\": \"memory\"}",
                "write /ctx/mounts/api {\"type\": \"http\", \"url\": \"https://api.example.com\"}",
                "write /ctx/mounts/data null"
            ]
        })
    }

    fn http_help(&self) -> JsonValue {
        json!({
            "title": "HTTP Brokers",
            "description": "HTTP brokers allow making requests to any URL.",
            "brokers": {
                "/ctx/http": "Async - requests execute in background threads",
                "/ctx/http_sync": "Sync - blocks until request completes on read"
            },
            "async_usage": {
                "step1": "Write an HttpRequest to /ctx/http",
                "step2": "Request starts executing immediately in background",
                "step3": "Read from handle to check status (pending/complete/failed)",
                "step4": "Read from handle/response to get the HttpResponse"
            },
            "sync_usage": {
                "step1": "Write an HttpRequest to /ctx/http_sync",
                "step2": "Read from the handle to execute and get response (blocks)"
            },
            "request_format": {
                "method": "GET | POST | PUT | DELETE | PATCH | HEAD | OPTIONS",
                "path": "Full URL (e.g., https://api.example.com/users)",
                "headers": "Optional object of header name -> value",
                "query": "Optional object of query param name -> value",
                "body": "Optional JSON body for POST/PUT/PATCH"
            },
            "examples": [
                {
                    "description": "Async: Queue multiple requests",
                    "commands": [
                        "write /ctx/http {\"path\": \"https://httpbin.org/delay/2\"}",
                        "write /ctx/http {\"path\": \"https://httpbin.org/delay/1\"}",
                        "read /ctx/http/outstanding/0  # Check status",
                        "read /ctx/http/outstanding/0/response  # Get response when complete"
                    ]
                },
                {
                    "description": "Sync: Simple blocking request",
                    "commands": [
                        "write /ctx/http_sync {\"path\": \"https://httpbin.org/get\"}",
                        "read /ctx/http_sync/outstanding/0  # Blocks until complete"
                    ]
                }
            ],
            "status_format": {
                "id": "Request ID",
                "state": "pending | complete | failed",
                "error": "Error message if failed",
                "response_path": "Path to read response from (when complete)"
            },
            "response_format": {
                "status": "HTTP status code (e.g., 200)",
                "status_text": "Status text (e.g., \"OK\")",
                "headers": "Response headers",
                "body": "Response body as JSON (or null if not JSON)",
                "body_text": "Raw response body as string"
            }
        })
    }

    fn paths_help(&self) -> JsonValue {
        json!({
            "title": "Path Syntax",
            "description": "Paths identify locations in the store tree.",
            "syntax": {
                "absolute": "/foo/bar - starts from root",
                "relative": "foo/bar - relative to current directory",
                "parent": "../foo - go up one level",
                "root": "/ - the root path"
            },
            "special_paths": {
                "/ctx/mounts": "Mount management",
                "/ctx/http": "HTTP broker (default mount)",
                "/ctx/help": "This help system"
            },
            "notes": [
                "Trailing slashes are normalized: /foo/ equals /foo",
                "Double slashes are normalized: /foo//bar equals /foo/bar",
                "Path components must be valid identifiers (letters, numbers, underscores)"
            ]
        })
    }

    fn examples_help(&self) -> JsonValue {
        json!({
            "title": "Usage Examples",
            "examples": [
                {
                    "title": "Create and use a memory store",
                    "steps": [
                        "write /ctx/mounts/data {\"type\": \"memory\"}",
                        "write /data/users/1 {\"name\": \"Alice\", \"email\": \"alice@example.com\"}",
                        "read /data/users/1",
                        "read /data/users"
                    ]
                },
                {
                    "title": "Make an HTTP request",
                    "steps": [
                        "write /ctx/http {\"method\": \"GET\", \"path\": \"https://httpbin.org/json\"}",
                        "read /ctx/http/outstanding/0"
                    ]
                },
                {
                    "title": "Mount a local directory",
                    "steps": [
                        "write /ctx/mounts/local {\"type\": \"local\", \"path\": \"/tmp/structfs-data\"}",
                        "write /local/config {\"setting\": \"value\"}",
                        "read /local/config"
                    ]
                }
            ]
        })
    }

    fn stores_help(&self) -> JsonValue {
        json!({
            "title": "Store Types",
            "stores": {
                "memory": {
                    "description": "In-memory JSON store, data is lost on exit",
                    "config": "{\"type\": \"memory\"}",
                    "use_case": "Temporary data, testing"
                },
                "local": {
                    "description": "JSON files stored on local filesystem",
                    "config": "{\"type\": \"local\", \"path\": \"/path/to/dir\"}",
                    "use_case": "Persistent local storage"
                },
                "http": {
                    "description": "HTTP client with a base URL",
                    "config": "{\"type\": \"http\", \"url\": \"https://api.example.com\"}",
                    "use_case": "REST API with fixed base URL"
                },
                "httpbroker": {
                    "description": "Sync HTTP broker - blocks on read until request completes",
                    "config": "{\"type\": \"httpbroker\"}",
                    "use_case": "Simple one-off HTTP requests"
                },
                "asynchttpbroker": {
                    "description": "Async HTTP broker - executes in background threads",
                    "config": "{\"type\": \"asynchttpbroker\"}",
                    "use_case": "Multiple concurrent requests, non-blocking"
                },
                "structfs": {
                    "description": "Remote StructFS server",
                    "config": "{\"type\": \"structfs\", \"url\": \"https://structfs.example.com\"}",
                    "use_case": "Connecting to another StructFS instance"
                }
            }
        })
    }

    fn registers_help(&self) -> JsonValue {
        json!({
            "title": "Registers",
            "description": "Registers are named storage locations that can hold JSON values from command output.",
            "syntax": {
                "@name": "Access register named 'name'",
                "@name/path": "Navigate into JSON structure stored in register"
            },
            "capture_output": {
                "format": "@name command [args]",
                "description": "Prefix any command with @name to store its output in a register",
                "examples": [
                    "@result read /some/path     - Store read output in 'result'",
                    "@data read /ctx/mounts         - Store mount list in 'data'"
                ]
            },
            "read_from_register": {
                "format": "read @name[/path]",
                "examples": [
                    "read @result               - Read entire register contents",
                    "read @result/nested/field  - Read sub-path within register"
                ]
            },
            "write_operations": {
                "write_to_register": {
                    "format": "write @name <json>",
                    "example": "write @temp {\"key\": \"value\"}"
                },
                "write_from_register": {
                    "format": "write <path> @source",
                    "example": "write /destination @source"
                },
                "copy_between_registers": {
                    "format": "write @dest @source",
                    "example": "write @backup @data"
                }
            },
            "commands": {
                "registers": "List all register names (alias: regs)"
            },
            "notes": [
                "Registers persist only for the current REPL session",
                "Register contents are stored as JSON values",
                "Non-JSON output is stored as a string"
            ]
        })
    }
}

impl Default for HelpStore<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for HelpStore<'_> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        let help = self.get_help(from);
        Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
            help,
        ))))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        let help = self.get_help(from);
        let record =
            serde_json::from_value(help).map_err(|e| StoreError::RecordDeserialization {
                message: e.to_string(),
            })?;
        Ok(Some(record))
    }
}

impl Writer for HelpStore<'_> {
    fn write<RecordType: Serialize>(
        &mut self,
        _destination: &Path,
        _data: RecordType,
    ) -> Result<Path, StoreError> {
        Err(StoreError::Raw {
            message: "Help store is read-only".to_string(),
        })
    }
}
