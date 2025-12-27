//! Help store that provides documentation index and REPL topics.
//!
//! Mount at `/ctx/help` to provide in-REPL documentation:
//! - `read /ctx/help` - Overview and topic list
//! - `read /ctx/help/commands` - Available commands
//! - `read /ctx/help/mounts` - Mount system documentation
//! - `read /ctx/help/http` - HTTP broker usage
//!
//! ## Docs Protocol
//!
//! Store-specific documentation is provided by stores themselves via a `docs` path.
//! When a store is mounted, the routing system automatically creates a redirect
//! from `/ctx/help/{name}` to `{mount}/docs` if the store provides docs.
//!
//! For example, mounting SysStore at `/ctx/sys` with docs at `/ctx/sys/docs`
//! creates a redirect: `/ctx/help/sys` -> `/ctx/sys/docs`.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// A store that provides help documentation index and REPL topics.
///
/// The HelpStore provides:
/// - General REPL usage documentation
/// - Topic index (commands, mounts, http, etc.)
///
/// Store-specific docs are accessed via redirects (handled by OverlayStore).
pub struct HelpStore;

impl HelpStore {
    pub fn new() -> Self {
        Self
    }

    fn get_help(&self, path: &Path) -> Value {
        if path.is_empty() {
            return self.root_help();
        }

        let full_path = path.components.join("/");
        match full_path.as_str() {
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

    fn root_help(&self) -> Value {
        let mut map = BTreeMap::new();
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

        let mut topics = BTreeMap::new();
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
        map.insert("topics".to_string(), Value::Map(topics));

        map.insert(
            "store_docs".to_string(),
            Value::String(
                "Store-specific docs are accessed via redirects. Try: read /ctx/help/sys"
                    .to_string(),
            ),
        );

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
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("REPL Commands".to_string()),
        );

        let mut commands = BTreeMap::new();
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
        let mut map = BTreeMap::new();
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

        let mut operations = BTreeMap::new();
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

        let mut configs = BTreeMap::new();
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
        map.insert("mount_configs".to_string(), Value::Map(configs));

        Value::Map(map)
    }

    fn http_help(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("HTTP Brokers".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("HTTP brokers allow making requests to any URL.".to_string()),
        );

        let mut brokers = BTreeMap::new();
        brokers.insert(
            "/ctx/http".to_string(),
            Value::String("Async - requests execute in background threads".to_string()),
        );
        brokers.insert(
            "/ctx/http_sync".to_string(),
            Value::String("Sync - blocks until request completes on read".to_string()),
        );
        map.insert("brokers".to_string(), Value::Map(brokers));

        let mut request_format = BTreeMap::new();
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
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Path Syntax".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("Paths identify locations in the store tree.".to_string()),
        );

        let mut syntax = BTreeMap::new();
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

        Value::Map(map)
    }

    fn examples_help(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Usage Examples".to_string()),
        );

        let mut example1 = BTreeMap::new();
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
            ]),
        );

        let mut example2 = BTreeMap::new();
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
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Store Types".to_string()),
        );

        let mut stores = BTreeMap::new();

        let mut memory = BTreeMap::new();
        memory.insert(
            "description".to_string(),
            Value::String("In-memory JSON store, data is lost on exit".to_string()),
        );
        memory.insert(
            "config".to_string(),
            Value::String("{\"type\": \"memory\"}".to_string()),
        );
        stores.insert("memory".to_string(), Value::Map(memory));

        let mut local = BTreeMap::new();
        local.insert(
            "description".to_string(),
            Value::String("JSON files stored on local filesystem".to_string()),
        );
        local.insert(
            "config".to_string(),
            Value::String("{\"type\": \"local\", \"path\": \"/path/to/dir\"}".to_string()),
        );
        stores.insert("local".to_string(), Value::Map(local));

        let mut http = BTreeMap::new();
        http.insert(
            "description".to_string(),
            Value::String("HTTP client with a base URL".to_string()),
        );
        http.insert(
            "config".to_string(),
            Value::String("{\"type\": \"http\", \"url\": \"https://api.example.com\"}".to_string()),
        );
        stores.insert("http".to_string(), Value::Map(http));

        map.insert("stores".to_string(), Value::Map(stores));

        Value::Map(map)
    }

    fn registers_help(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".to_string(), Value::String("Registers".to_string()));
        map.insert(
            "description".to_string(),
            Value::String(
                "Registers are named storage locations that can hold JSON values from command output."
                    .to_string(),
            ),
        );

        let mut syntax = BTreeMap::new();
        syntax.insert(
            "@name".to_string(),
            Value::String("Access register named 'name'".to_string()),
        );
        syntax.insert(
            "@name command".to_string(),
            Value::String("Capture command output in register".to_string()),
        );
        syntax.insert(
            "*@name".to_string(),
            Value::String("Dereference register as path".to_string()),
        );
        map.insert("syntax".to_string(), Value::Map(syntax));

        Value::Map(map)
    }

    fn suggest_help(&self, query: &str) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "error".to_string(),
            Value::String(format!("No help found for: '{}'", query)),
        );
        map.insert(
            "hint".to_string(),
            Value::String("Try one of the available topics below".to_string()),
        );

        let topics = vec![
            "commands",
            "mounts",
            "http",
            "paths",
            "registers",
            "examples",
            "stores",
        ];
        map.insert(
            "available_topics".to_string(),
            Value::Array(
                topics
                    .into_iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

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
    }

    #[test]
    fn test_default_impl() {
        let _help = HelpStore;
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
                assert!(map.contains_key("syntax"));
            }
            _ => panic!("Expected map"),
        }
    }
}
