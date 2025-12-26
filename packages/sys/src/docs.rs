//! Documentation store for sys primitives.

use std::collections::BTreeMap;

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
            "env" => Some(self.env_docs(&path.components[1..])),
            "time" => Some(self.time_docs(&path.components[1..])),
            "random" => Some(self.random_docs(&path.components[1..])),
            "proc" => Some(self.proc_docs(&path.components[1..])),
            "fs" => Some(self.fs_docs(&path.components[1..])),
            _ => None,
        }
    }

    fn root_docs(&self) -> Value {
        let mut subsystems = BTreeMap::new();
        subsystems.insert(
            "env".to_string(),
            Value::String("Environment variables - read, write, list".to_string()),
        );
        subsystems.insert(
            "time".to_string(),
            Value::String("Clocks and sleep - current time, monotonic, delays".to_string()),
        );
        subsystems.insert(
            "random".to_string(),
            Value::String("Random generation - integers, UUIDs, bytes".to_string()),
        );
        subsystems.insert(
            "proc".to_string(),
            Value::String("Process info - PID, CWD, args, environment".to_string()),
        );
        subsystems.insert(
            "fs".to_string(),
            Value::String("Filesystem - open, read, write, stat, mkdir, etc.".to_string()),
        );

        let examples = Value::Array(vec![
            Value::String("read env/HOME".to_string()),
            Value::String("read time/now".to_string()),
            Value::String("read random/uuid".to_string()),
            Value::String("read proc/self/pid".to_string()),
            Value::String(
                "write fs/open {\"path\": \"/tmp/test\", \"mode\": \"write\"}".to_string(),
            ),
        ]);

        let see_also = Value::Array(vec![
            Value::String("docs/env".to_string()),
            Value::String("docs/time".to_string()),
            Value::String("docs/random".to_string()),
            Value::String("docs/proc".to_string()),
            Value::String("docs/fs".to_string()),
        ]);

        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("System Primitives".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("OS primitives exposed through StructFS paths.".to_string()),
        );
        map.insert("subsystems".to_string(), Value::Map(subsystems));
        map.insert("examples".to_string(), examples);
        map.insert("see_also".to_string(), see_also);

        Value::Map(map)
    }

    fn env_docs(&self, _subpath: &[String]) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Environment Variables".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("Read and write process environment variables.".to_string()),
        );
        Value::Map(map)
    }

    fn time_docs(&self, _subpath: &[String]) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Time Operations".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("Clocks, timestamps, and delays.".to_string()),
        );
        Value::Map(map)
    }

    fn random_docs(&self, _subpath: &[String]) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Random Number Generation".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("Cryptographically secure random values.".to_string()),
        );
        Value::Map(map)
    }

    fn proc_docs(&self, _subpath: &[String]) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Process Information".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("Information about the current process.".to_string()),
        );
        Value::Map(map)
    }

    fn fs_docs(&self, _subpath: &[String]) -> Value {
        let mut map = BTreeMap::new();
        map.insert(
            "title".to_string(),
            Value::String("Filesystem Operations".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String("File and directory operations with handle-based I/O.".to_string()),
        );
        Value::Map(map)
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
        Err(Error::Other {
            message: "Documentation is read-only".to_string(),
        })
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
                assert!(map.contains_key("subsystems"));
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
            }
            _ => panic!("Expected map"),
        }
    }
}
