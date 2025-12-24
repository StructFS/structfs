//! Help store that provides documentation via read operations.
//!
//! Mount at `/ctx/help` to provide in-REPL documentation:
//! - `read /ctx/help` - Overview and topic list
//! - `read /ctx/help/commands` - Available commands
//! - `read /ctx/help/mounts` - Mount system documentation
//! - `read /ctx/help/http` - HTTP broker usage

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value as JsonValue};

use structfs_store::{Error as StoreError, Path, Reader, Writer};

/// A store that returns help documentation on read.
pub struct HelpStore;

impl HelpStore {
    pub fn new() -> Self {
        Self
    }

    fn get_help(&self, path: &Path) -> JsonValue {
        if path.is_empty() {
            return self.root_help();
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
            // Topic-based help (single component)
            _ => match path.components[0].as_str() {
                "commands" => self.commands_help(),
                "mounts" => self.mounts_help(),
                "http" => self.http_help(),
                "paths" => self.paths_help(),
                "examples" => self.examples_help(),
                "stores" => self.stores_help(),
                "registers" => self.registers_help(),
                topic => json!({
                    "error": format!("Unknown help topic: '{}'", topic),
                    "hint": "Use a topic name or a system path like 'ctx/http'",
                    "available_topics": ["commands", "mounts", "http", "paths", "examples", "stores", "registers"],
                    "system_paths": ["ctx", "ctx/http", "ctx/help", "ctx/mounts"]
                }),
            },
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
                "/ctx/mounts": "Mount management - create and manage store mounts"
            },
            "usage": [
                "read /ctx/help          - Get help",
                "read /ctx/help/http     - Help on HTTP broker",
                "write /ctx/http <req>   - Queue an HTTP request (async)",
                "read /ctx/http/outstanding/0         - Check status",
                "read /ctx/http/outstanding/0/response - Get response when complete"
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
                "stores": "Available store types"
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

impl Default for HelpStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for HelpStore {
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

impl Writer for HelpStore {
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
