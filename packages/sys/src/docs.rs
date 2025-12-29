//! Documentation store for sys primitives.

use collection_literals::btree;

use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};

/// Documentation store for sys primitives.
pub struct DocsStore;

impl DocsStore {
    pub fn new() -> Self {
        Self
    }

    fn get_docs(&self, path: &Path) -> Option<Value> {
        if path.is_empty() {
            return Some(self.root_docs());
        }

        match path[0].as_str() {
            "env" => Some(Self::env_docs()),
            "time" => Some(Self::time_docs()),
            "random" => Some(Self::random_docs()),
            "proc" => Some(Self::proc_docs()),
            "fs" => Some(Self::fs_docs()),
            _ => None,
        }
    }

    fn root_docs(&self) -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("System Primitives".into()),
            "description".into() => Value::String("OS primitives exposed through StructFS paths.".into()),
            "subsystems".into() => Value::Map(btree! {
                "env".into() => Value::String("Environment variables - read, write, list".into()),
                "time".into() => Value::String("Clocks and sleep - current time, monotonic, delays".into()),
                "random".into() => Value::String("Random generation - integers, UUIDs, bytes".into()),
                "proc".into() => Value::String("Process info - PID, CWD, args, environment".into()),
                "fs".into() => Value::String("Filesystem - open, read, write, stat, mkdir, etc.".into()),
            }),
            "examples".into() => Value::Array(vec![
                Value::String("read env/HOME".into()),
                Value::String("read time/now".into()),
                Value::String("read random/uuid".into()),
                Value::String("read proc/self/pid".into()),
                Value::String("write fs/open {\"path\": \"/tmp/test\", \"mode\": \"write\"}".into()),
            ]),
            "see_also".into() => Value::Array(vec![
                Value::String("docs/env".into()),
                Value::String("docs/time".into()),
                Value::String("docs/random".into()),
                Value::String("docs/proc".into()),
                Value::String("docs/fs".into()),
            ]),
        })
    }

    fn env_docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Environment Variables".into()),
            "description".into() => Value::String("Read and write process environment variables.".into()),
        })
    }

    fn time_docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Time Operations".into()),
            "description".into() => Value::String("Clocks, timestamps, and delays.".into()),
        })
    }

    fn random_docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Random Number Generation".into()),
            "description".into() => Value::String("Cryptographically secure random values.".into()),
        })
    }

    fn proc_docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Process Information".into()),
            "description".into() => Value::String("Information about the current process.".into()),
        })
    }

    fn fs_docs() -> Value {
        Value::Map(btree! {
            "title".into() => Value::String("Filesystem Operations".into()),
            "description".into() => Value::String("File and directory operations with handle-based I/O.".into()),
        })
    }
}

impl Default for DocsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for DocsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.get_docs(from).map(Record::parsed))
    }
}

impl Writer for DocsStore {
    fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
        Err(Error::store("docs", "write", "Documentation is read-only"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::{path, NoCodec};

    #[test]
    fn read_root() {
        let mut store = DocsStore::new();
        let record = store.read(&path!("")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
                assert!(map.contains_key("subsystems"));
                assert!(map.contains_key("examples"));
                assert!(map.contains_key("see_also"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_env_docs() {
        let mut store = DocsStore::new();
        let record = store.read(&path!("env")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_time_docs() {
        let mut store = DocsStore::new();
        let record = store.read(&path!("time")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_random_docs() {
        let mut store = DocsStore::new();
        let record = store.read(&path!("random")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_proc_docs() {
        let mut store = DocsStore::new();
        let record = store.read(&path!("proc")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_fs_docs() {
        let mut store = DocsStore::new();
        let record = store.read(&path!("fs")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
                assert!(map.contains_key("description"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let mut store = DocsStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_with_subpath() {
        let mut store = DocsStore::new();
        // Subpaths should still return docs for the main topic
        let record = store.read(&path!("env/subpath")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("title"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn write_returns_error() {
        let mut store = DocsStore::new();
        let result = store.write(&path!("test"), Record::parsed(Value::Null));
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::Store { message, .. } => {
                assert!(message.contains("read-only"));
            }
            _ => panic!("Expected Store error"),
        }
    }

    #[test]
    fn default_impl() {
        let _store: DocsStore = Default::default();
        // Just verify default works
    }
}
