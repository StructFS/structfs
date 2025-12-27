//! REPL documentation store.
//!
//! Mounted at `/ctx/repl`, with docs at `/ctx/repl/docs`.
//! Discovery creates redirect: `/ctx/help/repl` -> `/ctx/repl/docs`.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// Documentation for the REPL itself.
///
/// Provides documentation at the `/docs` sub-path:
/// - `/docs` - Root manifest with title, description, children
/// - `/docs/commands` - Command reference
/// - `/docs/registers` - Register syntax
/// - `/docs/paths` - Path syntax
/// - `/docs/mounts` - Mount system
/// - `/docs/examples` - Usage examples
pub struct ReplDocsStore {
    docs: BTreeMap<String, Value>,
}

impl ReplDocsStore {
    pub fn new() -> Self {
        let mut docs = BTreeMap::new();

        // Root manifest
        docs.insert(String::new(), Self::root_manifest());

        // Individual topics
        docs.insert("commands".into(), Self::commands_docs());
        docs.insert("registers".into(), Self::registers_docs());
        docs.insert("paths".into(), Self::paths_docs());
        docs.insert("examples".into(), Self::examples_docs());
        docs.insert("mounts".into(), Self::mounts_docs());

        Self { docs }
    }

    fn root_manifest() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("REPL Documentation".into()));
        map.insert(
            "description".into(),
            Value::String("Interactive command-line interface for StructFS".into()),
        );
        map.insert(
            "children".into(),
            Value::Array(vec![
                Value::String("commands".into()),
                Value::String("registers".into()),
                Value::String("paths".into()),
                Value::String("mounts".into()),
                Value::String("examples".into()),
            ]),
        );
        map.insert(
            "keywords".into(),
            Value::Array(vec![
                Value::String("repl".into()),
                Value::String("cli".into()),
                Value::String("terminal".into()),
                Value::String("interactive".into()),
            ]),
        );
        Value::Map(map)
    }

    fn commands_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Commands".into()));
        map.insert(
            "description".into(),
            Value::String("Available REPL commands and their syntax".into()),
        );

        let commands = vec![
            ("read", "read <path>", "Read value at path (alias: get, r)"),
            (
                "write",
                "write <path> <json>",
                "Write JSON value to path (alias: set, w)",
            ),
            ("ls", "ls [path]", "List children at path"),
            ("cd", "cd <path>", "Change current directory"),
            ("pwd", "pwd", "Print current directory"),
            ("mounts", "mounts", "List all mount points"),
            ("registers", "registers", "List all registers (alias: regs)"),
            ("help", "help [topic]", "Show help"),
            ("exit", "exit", "Exit the REPL (alias: quit, q)"),
        ];

        let command_list: Vec<Value> = commands
            .iter()
            .map(|(name, syntax, desc)| {
                let mut cmd = BTreeMap::new();
                cmd.insert("name".into(), Value::String(name.to_string()));
                cmd.insert("syntax".into(), Value::String(syntax.to_string()));
                cmd.insert("description".into(), Value::String(desc.to_string()));
                Value::Map(cmd)
            })
            .collect();

        map.insert("commands".into(), Value::Array(command_list));

        let mut aliases = BTreeMap::new();
        aliases.insert("r".into(), Value::String("read".into()));
        aliases.insert("get".into(), Value::String("read".into()));
        aliases.insert("w".into(), Value::String("write".into()));
        aliases.insert("set".into(), Value::String("write".into()));
        aliases.insert("regs".into(), Value::String("registers".into()));
        aliases.insert("quit".into(), Value::String("exit".into()));
        aliases.insert("q".into(), Value::String("exit".into()));
        map.insert("aliases".into(), Value::Map(aliases));

        Value::Map(map)
    }

    fn registers_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Registers".into()));
        map.insert(
            "description".into(),
            Value::String("Named storage for command outputs".into()),
        );

        let mut syntax = BTreeMap::new();
        syntax.insert(
            "capture".into(),
            Value::String("@name <command> - Store command output in register".into()),
        );
        syntax.insert(
            "read".into(),
            Value::String("read @name - Read register value".into()),
        );
        syntax.insert(
            "dereference".into(),
            Value::String("*@name - Use register value as path".into()),
        );
        syntax.insert(
            "write".into(),
            Value::String("write @name <value> - Set register directly".into()),
        );
        map.insert("syntax".into(), Value::Map(syntax));

        let examples = [
            "@result read /ctx/sys/time/now",
            "read @result",
            "@path read /ctx/sys/env/HOME",
            "read *@path",
        ];
        map.insert(
            "examples".into(),
            Value::Array(
                examples
                    .iter()
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
        );

        Value::Map(map)
    }

    fn paths_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Path Syntax".into()));
        map.insert(
            "description".into(),
            Value::String("How paths work in StructFS".into()),
        );

        let rules = [
            "Paths are slash-separated components",
            "Leading slash is optional",
            "Components must be valid identifiers or integers",
            "Trailing slashes are normalized away",
            "Empty components (//) are normalized",
        ];
        map.insert(
            "rules".into(),
            Value::Array(rules.iter().map(|s| Value::String(s.to_string())).collect()),
        );

        let examples = [
            ("/ctx/sys/time/now", "Absolute path"),
            ("ctx/sys/time/now", "Same path without leading slash"),
            ("data/users/0", "Numeric component for array access"),
        ];
        let example_list: Vec<Value> = examples
            .iter()
            .map(|(path, desc)| {
                let mut ex = BTreeMap::new();
                ex.insert("path".into(), Value::String(path.to_string()));
                ex.insert("description".into(), Value::String(desc.to_string()));
                Value::Map(ex)
            })
            .collect();
        map.insert("examples".into(), Value::Array(example_list));

        Value::Map(map)
    }

    fn mounts_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Mount System".into()));
        map.insert(
            "description".into(),
            Value::String("How stores are mounted and managed".into()),
        );

        let mut operations = BTreeMap::new();
        operations.insert(
            "list".into(),
            Value::String("read /ctx/mounts - List all mounts".into()),
        );
        operations.insert(
            "mount".into(),
            Value::String("write /ctx/mounts/<name> {\"type\": \"memory\"} - Create mount".into()),
        );
        operations.insert(
            "unmount".into(),
            Value::String("write /ctx/mounts/<name> null - Remove mount".into()),
        );
        operations.insert(
            "inspect".into(),
            Value::String("read /ctx/mounts/<name> - Get mount config".into()),
        );
        map.insert("operations".into(), Value::Map(operations));

        let mount_types = [
            ("memory", "In-memory JSON store"),
            ("local", "Local filesystem directory"),
            ("http", "HTTP client to base URL"),
            ("httpbroker", "Sync HTTP request broker"),
            ("asynchttpbroker", "Async HTTP request broker"),
        ];
        let type_list: Vec<Value> = mount_types
            .iter()
            .map(|(name, desc)| {
                let mut t = BTreeMap::new();
                t.insert("type".into(), Value::String(name.to_string()));
                t.insert("description".into(), Value::String(desc.to_string()));
                Value::Map(t)
            })
            .collect();
        map.insert("types".into(), Value::Array(type_list));

        Value::Map(map)
    }

    fn examples_docs() -> Value {
        let mut map = BTreeMap::new();
        map.insert("title".into(), Value::String("Examples".into()));
        map.insert(
            "description".into(),
            Value::String("Common usage patterns".into()),
        );

        let examples: &[(&str, &[&str])] = &[
            ("Read system time", &["read /ctx/sys/time/now"]),
            (
                "Make HTTP request",
                &[
                    "@req write /ctx/http {\"method\": \"GET\", \"path\": \"https://api.example.com/data\"}",
                    "read *@req",
                ],
            ),
            (
                "Create and use a store",
                &[
                    "write /ctx/mounts/mydata {\"type\": \"memory\"}",
                    "write /mydata/users/alice {\"name\": \"Alice\", \"age\": 30}",
                    "read /mydata/users/alice",
                ],
            ),
            (
                "Work with registers",
                &["@home read /ctx/sys/env/HOME", "read @home"],
            ),
        ];

        let example_list: Vec<Value> = examples
            .iter()
            .map(|(title, commands)| {
                let mut ex = BTreeMap::new();
                ex.insert("title".into(), Value::String(title.to_string()));
                ex.insert(
                    "commands".into(),
                    Value::Array(
                        commands
                            .iter()
                            .map(|c| Value::String(c.to_string()))
                            .collect(),
                    ),
                );
                Value::Map(ex)
            })
            .collect();

        map.insert("examples".into(), Value::Array(example_list));
        Value::Map(map)
    }
}

impl Default for ReplDocsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for ReplDocsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Must be under /docs path
        if from.is_empty() {
            return Ok(None); // Root of ReplStore, not docs
        }

        if from[0] != "docs" {
            return Ok(None); // Not a docs path
        }

        // Strip "docs" prefix
        let doc_path = if from.len() > 1 {
            from.components[1..].join("/")
        } else {
            String::new()
        };

        Ok(self.docs.get(&doc_path).cloned().map(Record::parsed))
    }
}

impl Writer for ReplDocsStore {
    fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::store(
            "repl_docs",
            "write",
            "REPL docs are read-only",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, NoCodec};

    #[test]
    fn repl_docs_has_root_manifest() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert_eq!(
                    map.get("title"),
                    Some(&Value::String("REPL Documentation".into()))
                );
                assert!(map.contains_key("children"));
                assert!(map.contains_key("keywords"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn repl_docs_has_commands() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs/commands")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert!(map.contains_key("commands"));
                assert!(map.contains_key("aliases"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn repl_docs_has_registers() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs/registers")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert!(map.contains_key("syntax"));
                assert!(map.contains_key("examples"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn repl_docs_has_paths() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs/paths")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert!(map.contains_key("rules"));
                assert!(map.contains_key("examples"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn repl_docs_has_mounts() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs/mounts")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert!(map.contains_key("operations"));
                assert!(map.contains_key("types"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn repl_docs_has_examples() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs/examples")).unwrap().unwrap();
        let value = result.into_value(&NoCodec).unwrap();

        match value {
            Value::Map(map) => {
                assert!(map.contains_key("examples"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn repl_docs_root_returns_none() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn repl_docs_non_docs_path_returns_none() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("other")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn repl_docs_unknown_topic_returns_none() {
        let mut store = ReplDocsStore::new();
        let result = store.read(&path!("docs/unknown")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn repl_docs_is_read_only() {
        let mut store = ReplDocsStore::new();
        let result = store.write(&path!("docs/test"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn repl_docs_default() {
        let store: ReplDocsStore = Default::default();
        assert!(!store.docs.is_empty());
    }
}
