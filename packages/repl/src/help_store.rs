//! Help store that provides documentation via read operations (new architecture).
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

use structfs_core_store::overlay_store::StoreBox;
use structfs_core_store::{
    overlay_store::OverlayStore, Error, Path, Reader, Record, Value, Writer,
};

/// A store that returns help documentation on read (new architecture).
///
/// The HelpStore combines:
/// 1. Built-in help topics (commands, mounts, http, etc.)
/// 2. Mounted documentation from other stores (via the docs protocol)
///
/// When reading a path, it first checks if there's a mounted docs store
/// for that path prefix, then falls back to built-in help.
pub struct HelpStore {
    /// Mounted docs from other stores
    mounted_docs: OverlayStore,
    /// Track which topics have mounted docs
    mounted_topics: Vec<String>,
}

impl HelpStore {
    pub fn new() -> Self {
        Self {
            mounted_docs: OverlayStore::new(),
            mounted_topics: Vec::new(),
        }
    }

    /// Mount a store's documentation at a topic path.
    ///
    /// For example, `mount_docs("sys", sys_docs_store)` makes the sys store's
    /// documentation available at `help/sys`.
    pub fn mount_docs(&mut self, topic: &str, docs_store: StoreBox) {
        let path = Path::parse(topic).expect("valid topic path");
        self.mounted_docs.add_layer(path, docs_store);
        self.mounted_topics.push(topic.to_string());
    }

    /// Check if a topic has mounted documentation
    fn has_mounted_docs(&self, topic: &str) -> bool {
        self.mounted_topics.iter().any(|t| t == topic)
    }

    /// Try to read from mounted docs for a topic
    fn try_mounted_docs(&mut self, path: &Path) -> Option<Value> {
        if path.is_empty() {
            return None;
        }

        let topic = &path[0];
        if !self.has_mounted_docs(topic.as_str()) {
            return None;
        }

        // Try to read from mounted docs
        match self.mounted_docs.read(path) {
            Ok(Some(record)) => record.into_value(&structfs_core_store::NoCodec).ok(),
            _ => None,
        }
    }

    fn get_help(&mut self, path: &Path) -> Value {
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

                match path[0].as_str() {
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
    fn store_docs_redirect(&self, store_path: &str) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "message".to_string(),
            Value::String("This store provides its own documentation.".to_string()),
        );
        map.insert(
            "read_docs".to_string(),
            Value::String(format!("read /{}/docs", store_path)),
        );
        map.insert(
            "hint".to_string(),
            Value::String(
                "Stores that support the docs protocol expose documentation at their 'docs' path."
                    .to_string(),
            ),
        );
        Value::Map(map)
    }

    /// Redirect to a store's own documentation with a subpath
    fn store_docs_redirect_with_path(&self, store_path: &str, subpath: &str) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "message".to_string(),
            Value::String("This store provides its own documentation.".to_string()),
        );
        map.insert(
            "read_docs".to_string(),
            Value::String(format!("read /{}/docs/{}", store_path, subpath)),
        );
        map.insert(
            "hint".to_string(),
            Value::String(
                "Stores that support the docs protocol expose documentation at their 'docs' path."
                    .to_string(),
            ),
        );
        Value::Map(map)
    }

    fn ctx_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Context Directory (/ctx)".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("The /ctx directory contains built-in system stores.".to_string()),
        );

        let mut mounts = std::collections::BTreeMap::new();
        mounts.insert(
            "/ctx/http".to_string(),
            Value::String("Async HTTP broker - requests execute in background".to_string()),
        );
        mounts.insert(
            "/ctx/http_sync".to_string(),
            Value::String("Sync HTTP broker - blocks on read until complete".to_string()),
        );
        mounts.insert(
            "/ctx/help".to_string(),
            Value::String("This help system".to_string()),
        );
        mounts.insert(
            "/ctx/mounts".to_string(),
            Value::String("Mount management - create and manage store mounts".to_string()),
        );
        mounts.insert(
            "/ctx/sys".to_string(),
            Value::String("System primitives (env, time, proc, fs, random)".to_string()),
        );
        map.insert("mounts".to_string(), Value::Map(mounts));

        let usage = vec![
            "read /ctx/help          - Get help",
            "read /ctx/help/http     - Help on HTTP broker",
            "write /ctx/http <req>   - Queue an HTTP request (async)",
            "read /ctx/http/outstanding/0         - Check status",
            "read /ctx/http/outstanding/0/response - Get response when complete",
            "read /ctx/sys/env/HOME  - Read environment variable",
            "read /ctx/sys/time/now  - Get current time",
        ];
        map.insert(
            "usage".to_string(),
            Value::Array(
                usage
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn root_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("StructFS REPL Help".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String(
                "StructFS provides a uniform interface for accessing data through read/write operations on paths."
                    .to_string(),
            ),
        );

        let mut topics = std::collections::BTreeMap::new();
        topics.insert(
            "commands".to_string(),
            Value::String("Available REPL commands".to_string()),
        );
        topics.insert(
            "mounts".to_string(),
            Value::String("Mounting and managing stores".to_string()),
        );
        topics.insert(
            "http".to_string(),
            Value::String("Making HTTP requests".to_string()),
        );
        topics.insert(
            "paths".to_string(),
            Value::String("Path syntax and navigation".to_string()),
        );
        topics.insert(
            "registers".to_string(),
            Value::String("Registers for storing command output".to_string()),
        );
        topics.insert(
            "examples".to_string(),
            Value::String("Usage examples".to_string()),
        );
        topics.insert(
            "stores".to_string(),
            Value::String("Available store types".to_string()),
        );
        topics.insert(
            "sys".to_string(),
            Value::String("System primitives (env, time, proc, fs, random)".to_string()),
        );
        map.insert("topics".to_string(), Value::Map(topics));

        let quick_start = vec![
            "read /ctx/mounts          - List current mounts",
            "write /ctx/mounts/data {\"type\": \"memory\"}  - Create a memory store at /data",
            "write /data/hello {\"message\": \"world\"}  - Write data",
            "read /data/hello       - Read data back",
        ];
        map.insert(
            "quick_start".to_string(),
            Value::Array(
                quick_start
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn commands_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("REPL Commands".to_string()),
        );

        let mut commands = std::collections::BTreeMap::new();
        commands.insert(
            "read <path>".to_string(),
            Value::String("Read data from a path (aliases: get, r)".to_string()),
        );
        commands.insert(
            "write <path> <json>".to_string(),
            Value::String("Write JSON data to a path (aliases: set, w)".to_string()),
        );
        commands.insert(
            "cd <path>".to_string(),
            Value::String("Change current directory".to_string()),
        );
        commands.insert(
            "pwd".to_string(),
            Value::String("Print current directory".to_string()),
        );
        commands.insert(
            "mounts".to_string(),
            Value::String("List all current mounts (shortcut for read /ctx/mounts)".to_string()),
        );
        commands.insert(
            "help".to_string(),
            Value::String("Show help message".to_string()),
        );
        commands.insert(
            "exit".to_string(),
            Value::String("Exit the REPL (aliases: quit)".to_string()),
        );
        map.insert("commands".to_string(), Value::Map(commands));

        let examples = vec![
            "read /ctx/help",
            "write /ctx/mounts/test {\"type\": \"memory\"}",
            "cd /test",
            "write foo {\"bar\": 123}",
            "read foo",
        ];
        map.insert(
            "examples".to_string(),
            Value::Array(
                examples
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn mounts_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Mount System".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String(
                "Mounts attach stores to paths in the filesystem tree. Manage mounts through /ctx/mounts."
                    .to_string(),
            ),
        );

        let mut operations = std::collections::BTreeMap::new();
        operations.insert(
            "read /ctx/mounts".to_string(),
            Value::String("List all mounts".to_string()),
        );
        operations.insert(
            "read /ctx/mounts/<name>".to_string(),
            Value::String("Get config for a specific mount".to_string()),
        );
        operations.insert(
            "write /ctx/mounts/<name> <config>".to_string(),
            Value::String("Create or update a mount".to_string()),
        );
        operations.insert(
            "write /ctx/mounts/<name> null".to_string(),
            Value::String("Unmount a store".to_string()),
        );
        map.insert("operations".to_string(), Value::Map(operations));

        let mut configs = std::collections::BTreeMap::new();
        configs.insert(
            "memory".to_string(),
            Value::String("{\"type\": \"memory\"}".to_string()),
        );
        configs.insert(
            "local".to_string(),
            Value::String("{\"type\": \"local\", \"path\": \"/path/to/dir\"}".to_string()),
        );
        configs.insert(
            "http".to_string(),
            Value::String("{\"type\": \"http\", \"url\": \"https://api.example.com\"}".to_string()),
        );
        configs.insert(
            "httpbroker".to_string(),
            Value::String("{\"type\": \"httpbroker\"} (sync)".to_string()),
        );
        configs.insert(
            "asynchttpbroker".to_string(),
            Value::String(
                "{\"type\": \"asynchttpbroker\"} (async, background threads)".to_string(),
            ),
        );
        configs.insert(
            "structfs".to_string(),
            Value::String(
                "{\"type\": \"structfs\", \"url\": \"https://structfs.example.com\"}".to_string(),
            ),
        );
        map.insert("mount_configs".to_string(), Value::Map(configs));

        let examples = vec![
            "write /ctx/mounts/data {\"type\": \"memory\"}",
            "write /ctx/mounts/api {\"type\": \"http\", \"url\": \"https://api.example.com\"}",
            "write /ctx/mounts/data null",
        ];
        map.insert(
            "examples".to_string(),
            Value::Array(
                examples
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn http_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("HTTP Brokers".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("HTTP brokers allow making requests to any URL.".to_string()),
        );

        let mut brokers = std::collections::BTreeMap::new();
        brokers.insert(
            "/ctx/http".to_string(),
            Value::String("Async - requests execute in background threads".to_string()),
        );
        brokers.insert(
            "/ctx/http_sync".to_string(),
            Value::String("Sync - blocks until request completes on read".to_string()),
        );
        map.insert("brokers".to_string(), Value::Map(brokers));

        let mut async_usage = std::collections::BTreeMap::new();
        async_usage.insert(
            "step1".to_string(),
            Value::String("Write an HttpRequest to /ctx/http".to_string()),
        );
        async_usage.insert(
            "step2".to_string(),
            Value::String("Request starts executing immediately in background".to_string()),
        );
        async_usage.insert(
            "step3".to_string(),
            Value::String("Read from handle to check status (pending/complete/failed)".to_string()),
        );
        async_usage.insert(
            "step4".to_string(),
            Value::String("Read from handle/response to get the HttpResponse".to_string()),
        );
        map.insert("async_usage".to_string(), Value::Map(async_usage));

        let mut sync_usage = std::collections::BTreeMap::new();
        sync_usage.insert(
            "step1".to_string(),
            Value::String("Write an HttpRequest to /ctx/http_sync".to_string()),
        );
        sync_usage.insert(
            "step2".to_string(),
            Value::String("Read from the handle to execute and get response (blocks)".to_string()),
        );
        map.insert("sync_usage".to_string(), Value::Map(sync_usage));

        let mut request_format = std::collections::BTreeMap::new();
        request_format.insert(
            "method".to_string(),
            Value::String("GET | POST | PUT | DELETE | PATCH | HEAD | OPTIONS".to_string()),
        );
        request_format.insert(
            "path".to_string(),
            Value::String("Full URL (e.g., https://api.example.com/users)".to_string()),
        );
        request_format.insert(
            "headers".to_string(),
            Value::String("Optional object of header name -> value".to_string()),
        );
        request_format.insert(
            "query".to_string(),
            Value::String("Optional object of query param name -> value".to_string()),
        );
        request_format.insert(
            "body".to_string(),
            Value::String("Optional JSON body for POST/PUT/PATCH".to_string()),
        );
        map.insert("request_format".to_string(), Value::Map(request_format));

        Value::Map(map)
    }

    fn paths_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Path Syntax".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("Paths identify locations in the store tree.".to_string()),
        );

        let mut syntax = std::collections::BTreeMap::new();
        syntax.insert(
            "absolute".to_string(),
            Value::String("/foo/bar - starts from root".to_string()),
        );
        syntax.insert(
            "relative".to_string(),
            Value::String("foo/bar - relative to current directory".to_string()),
        );
        syntax.insert(
            "parent".to_string(),
            Value::String("../foo - go up one level".to_string()),
        );
        syntax.insert(
            "root".to_string(),
            Value::String("/ - the root path".to_string()),
        );
        map.insert("syntax".to_string(), Value::Map(syntax));

        let notes = vec![
            "Trailing slashes are normalized: /foo/ equals /foo",
            "Double slashes are normalized: /foo//bar equals /foo/bar",
            "Path components must be valid identifiers (letters, numbers, underscores)",
        ];
        map.insert(
            "notes".to_string(),
            Value::Array(
                notes
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn examples_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Usage Examples".to_string()),
        );

        let mut example1 = std::collections::BTreeMap::new();
        example1.insert(
            "title".to_string(),
            Value::String("Create and use a memory store".to_string()),
        );
        example1.insert(
            "steps".to_string(),
            Value::Array(vec![
                Value::String("write /ctx/mounts/data {\"type\": \"memory\"}".to_string()),
                Value::String(
                    "write /data/users/1 {\"name\": \"Alice\", \"email\": \"alice@example.com\"}"
                        .to_string(),
                ),
                Value::String("read /data/users/1".to_string()),
                Value::String("read /data/users".to_string()),
            ]),
        );

        let mut example2 = std::collections::BTreeMap::new();
        example2.insert(
            "title".to_string(),
            Value::String("Make an HTTP request".to_string()),
        );
        example2.insert(
            "steps".to_string(),
            Value::Array(vec![
                Value::String(
                    "write /ctx/http {\"method\": \"GET\", \"path\": \"https://httpbin.org/json\"}"
                        .to_string(),
                ),
                Value::String("read /ctx/http/outstanding/0".to_string()),
            ]),
        );

        map.insert(
            "examples".to_string(),
            Value::Array(vec![Value::Map(example1), Value::Map(example2)]),
        );

        Value::Map(map)
    }

    fn stores_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Store Types".to_string()),
        );

        let mut stores = std::collections::BTreeMap::new();

        let mut memory = std::collections::BTreeMap::new();
        memory.insert(
            "description".to_string(),
            Value::String("In-memory JSON store, data is lost on exit".to_string()),
        );
        memory.insert(
            "config".to_string(),
            Value::String("{\"type\": \"memory\"}".to_string()),
        );
        memory.insert(
            "use_case".to_string(),
            Value::String("Temporary data, testing".to_string()),
        );
        stores.insert("memory".to_string(), Value::Map(memory));

        let mut local = std::collections::BTreeMap::new();
        local.insert(
            "description".to_string(),
            Value::String("JSON files stored on local filesystem".to_string()),
        );
        local.insert(
            "config".to_string(),
            Value::String("{\"type\": \"local\", \"path\": \"/path/to/dir\"}".to_string()),
        );
        local.insert(
            "use_case".to_string(),
            Value::String("Persistent local storage".to_string()),
        );
        stores.insert("local".to_string(), Value::Map(local));

        let mut http = std::collections::BTreeMap::new();
        http.insert(
            "description".to_string(),
            Value::String("HTTP client with a base URL".to_string()),
        );
        http.insert(
            "config".to_string(),
            Value::String("{\"type\": \"http\", \"url\": \"https://api.example.com\"}".to_string()),
        );
        http.insert(
            "use_case".to_string(),
            Value::String("REST API with fixed base URL".to_string()),
        );
        stores.insert("http".to_string(), Value::Map(http));

        map.insert("stores".to_string(), Value::Map(stores));

        Value::Map(map)
    }

    fn registers_help(&self) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert("title".to_string(), Value::String("Registers".to_string()));
        map.insert(
            "description".to_string(),
            Value::String(
                "Registers are named storage locations that can hold JSON values from command output."
                    .to_string(),
            ),
        );

        let mut syntax = std::collections::BTreeMap::new();
        syntax.insert(
            "@name".to_string(),
            Value::String("Access register named 'name'".to_string()),
        );
        syntax.insert(
            "@name/path".to_string(),
            Value::String("Navigate into JSON structure stored in register".to_string()),
        );
        map.insert("syntax".to_string(), Value::Map(syntax));

        let mut capture = std::collections::BTreeMap::new();
        capture.insert(
            "format".to_string(),
            Value::String("@name command [args]".to_string()),
        );
        capture.insert(
            "description".to_string(),
            Value::String(
                "Prefix any command with @name to store its output in a register".to_string(),
            ),
        );
        capture.insert(
            "examples".to_string(),
            Value::Array(vec![
                Value::String(
                    "@result read /some/path     - Store read output in 'result'".to_string(),
                ),
                Value::String(
                    "@data read /ctx/mounts         - Store mount list in 'data'".to_string(),
                ),
            ]),
        );
        map.insert("capture_output".to_string(), Value::Map(capture));

        let notes = vec![
            "Registers persist only for the current REPL session",
            "Register contents are stored as JSON values",
            "Non-JSON output is stored as a string",
        ];
        map.insert(
            "notes".to_string(),
            Value::Array(
                notes
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn suggest_help(&self, query: &str) -> Value {
        let mut map = std::collections::BTreeMap::new();
        map.insert(
            "error".to_string(),
            Value::String(format!("No help found for: '{}'", query)),
        );
        map.insert(
            "hint".to_string(),
            Value::String("Try one of the available topics below".to_string()),
        );

        let mut available = std::collections::BTreeMap::new();
        available.insert(
            "commands".to_string(),
            Value::String("REPL commands (read, write, cd, etc.)".to_string()),
        );
        available.insert(
            "mounts".to_string(),
            Value::String("Mounting and managing stores".to_string()),
        );
        available.insert(
            "http".to_string(),
            Value::String("Making HTTP requests".to_string()),
        );
        available.insert(
            "paths".to_string(),
            Value::String("Path syntax and navigation".to_string()),
        );
        available.insert(
            "registers".to_string(),
            Value::String("Capturing and using command output (@name, *@name)".to_string()),
        );
        available.insert(
            "examples".to_string(),
            Value::String("Usage examples".to_string()),
        );
        available.insert(
            "stores".to_string(),
            Value::String("Available store types".to_string()),
        );
        available.insert(
            "sys".to_string(),
            Value::String("System primitives (env, time, fs, proc, random)".to_string()),
        );
        map.insert("available_topics".to_string(), Value::Map(available));

        Value::Map(map)
    }
}

impl Default for HelpStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for HelpStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let help = self.get_help(from);
        Ok(Some(Record::parsed(help)))
    }
}

impl Writer for HelpStore {
    fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::store("help", "write", "Help store is read-only"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_help(help: &mut HelpStore, path: &str) -> Value {
        let result = help.read(&Path::parse(path).unwrap()).unwrap();
        result
            .unwrap()
            .into_value(&structfs_core_store::NoCodec)
            .unwrap()
    }

    #[test]
    fn test_help_root() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("topics"));
                assert!(map.contains_key("quick_start"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_commands() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "commands");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("commands"));
                assert!(map.contains_key("examples"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_mounts() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "mounts");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("operations"));
                assert!(map.contains_key("mount_configs"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_http() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "http");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("brokers"));
                assert!(map.contains_key("async_usage"));
                assert!(map.contains_key("sync_usage"));
                assert!(map.contains_key("request_format"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_paths() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "paths");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("syntax"));
                assert!(map.contains_key("notes"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_examples() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "examples");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("examples"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_stores() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "stores");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("stores"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_registers() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "registers");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
                assert!(map.contains_key("syntax"));
                assert!(map.contains_key("capture_output"));
                assert!(map.contains_key("notes"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_ctx() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "ctx");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("mounts"));
                assert!(map.contains_key("usage"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_ctx_http() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "ctx/http");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("brokers"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_ctx_help() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "ctx/help");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("topics"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_ctx_mounts() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "ctx/mounts");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("operations"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_ctx_sys_no_mounted_docs() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "ctx/sys");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("message"));
                assert!(map.contains_key("read_docs"));
                assert!(map.contains_key("hint"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_ctx_sys_subpath_no_mounted_docs() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "ctx/sys/time");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("message"));
                assert!(map.contains_key("read_docs"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_unknown_topic() {
        let mut help = HelpStore::new();
        let value = read_help(&mut help, "nonexistent");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("error"));
                assert!(map.contains_key("hint"));
                assert!(map.contains_key("available_topics"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_help_read_only() {
        let mut help = HelpStore::new();
        let result = help.write(&Path::parse("test").unwrap(), Record::parsed(Value::Null));
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            Error::Store { message, .. } => {
                assert!(message.contains("read-only"));
            }
            _ => panic!("Expected Store error"),
        }
    }

    #[test]
    fn test_default_impl() {
        let help = HelpStore::default();
        assert!(help.mounted_topics.is_empty());
    }

    #[test]
    fn test_mount_docs() {
        use structfs_core_store::overlay_store::OnlyReadable;

        // Create a simple mock docs store
        struct MockDocsStore;
        impl Reader for MockDocsStore {
            fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
                let mut map = std::collections::BTreeMap::new();
                map.insert(
                    "docs".to_string(),
                    Value::String("Mock documentation".to_string()),
                );
                Ok(Some(Record::parsed(Value::Map(map))))
            }
        }

        let mut help = HelpStore::new();
        help.mount_docs("sys", Box::new(OnlyReadable::new(MockDocsStore)));

        // Verify the topic was mounted
        assert!(help.has_mounted_docs("sys"));
        assert!(!help.has_mounted_docs("other"));

        // Now reading sys should use mounted docs
        let value = read_help(&mut help, "sys");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("docs"));
            }
            _ => panic!("Expected map from mounted docs"),
        }
    }

    #[test]
    fn test_ctx_sys_with_mounted_docs() {
        use structfs_core_store::overlay_store::OnlyReadable;

        struct MockDocsStore;
        impl Reader for MockDocsStore {
            fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
                let mut map = std::collections::BTreeMap::new();
                map.insert(
                    "sys_docs".to_string(),
                    Value::String("System docs".to_string()),
                );
                Ok(Some(Record::parsed(Value::Map(map))))
            }
        }

        let mut help = HelpStore::new();
        help.mount_docs("sys", Box::new(OnlyReadable::new(MockDocsStore)));

        let value = read_help(&mut help, "ctx/sys");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("sys_docs"));
            }
            _ => panic!("Expected map from mounted docs"),
        }
    }

    #[test]
    fn test_ctx_sys_subpath_with_mounted_docs() {
        use structfs_core_store::overlay_store::OnlyReadable;

        struct MockDocsStore;
        impl Reader for MockDocsStore {
            fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
                if !from.is_empty() && from[0].as_str() == "time" {
                    let mut map = std::collections::BTreeMap::new();
                    map.insert(
                        "time_docs".to_string(),
                        Value::String("Time documentation".to_string()),
                    );
                    Ok(Some(Record::parsed(Value::Map(map))))
                } else {
                    Ok(None)
                }
            }
        }

        let mut help = HelpStore::new();
        help.mount_docs("sys", Box::new(OnlyReadable::new(MockDocsStore)));

        let value = read_help(&mut help, "ctx/sys/time");
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("time_docs"));
            }
            _ => panic!("Expected map from mounted docs"),
        }
    }

    #[test]
    fn test_try_mounted_docs_returns_none_for_read_error() {
        use structfs_core_store::overlay_store::OnlyReadable;

        struct ErrorDocsStore;
        impl Reader for ErrorDocsStore {
            fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
                Err(Error::store("test", "read", "Read error"))
            }
        }

        let mut help = HelpStore::new();
        help.mount_docs("error", Box::new(OnlyReadable::new(ErrorDocsStore)));

        // Should fall back to suggest_help since mounted docs returns error
        let value = read_help(&mut help, "error");
        match value {
            Value::Map(map) => {
                // Falls through to suggest_help
                assert!(map.contains_key("error") || map.contains_key("hint"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn test_try_mounted_docs_empty_path_returns_none() {
        let mut help = HelpStore::new();
        let result = help.try_mounted_docs(&Path::parse("").unwrap());
        assert!(result.is_none());
    }
}
