//! Process information store.

use collection_literals::btree;
use std::collections::BTreeMap;

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// Store for process information.
pub struct ProcStore;

impl ProcStore {
    pub fn new() -> Self {
        Self
    }

    fn self_info() -> Value {
        Value::Map(btree! {
            "pid".into() => Value::String("Current process ID".into()),
            "cwd".into() => Value::String("Current working directory".into()),
            "args".into() => Value::String("Command line arguments".into()),
            "exe".into() => Value::String("Path to current executable".into()),
            "env".into() => Value::String("Environment variables".into()),
        })
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            return Ok(Some(Value::Map(btree! {
                "self".into() => Self::self_info(),
            })));
        }

        // Must start with "self"
        if path[0].as_str() != "self" {
            return Ok(None);
        }

        if path.len() == 1 {
            return Ok(Some(Self::self_info()));
        }

        if path.len() != 2 {
            return Ok(None);
        }

        match path[1].as_str() {
            "pid" => Ok(Some(Value::Integer(std::process::id() as i64))),
            "cwd" => match std::env::current_dir() {
                Ok(cwd) => Ok(Some(Value::String(cwd.to_string_lossy().to_string()))),
                Err(e) => Err(Error::Io(e)),
            },
            "args" => {
                let args: Vec<Value> = std::env::args().map(Value::String).collect();
                Ok(Some(Value::Array(args)))
            }
            "exe" => match std::env::current_exe() {
                Ok(exe) => Ok(Some(Value::String(exe.to_string_lossy().to_string()))),
                Err(e) => Err(Error::Io(e)),
            },
            "env" => {
                let vars: BTreeMap<String, Value> = std::env::vars()
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect();
                Ok(Some(Value::Map(vars)))
            }
            _ => Ok(None),
        }
    }
}

impl Default for ProcStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for ProcStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for ProcStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // Must be proc/self/...
        if to.len() != 2 || to[0].as_str() != "self" {
            return Err(Error::store("proc", "write", "Invalid proc path"));
        }

        match to[1].as_str() {
            "cwd" => {
                let value = data.into_value(&NoCodec)?;

                let new_cwd = match &value {
                    Value::String(s) => s.as_str(),
                    _ => {
                        return Err(Error::store("proc", "cwd", "cwd must be a string path"));
                    }
                };

                std::env::set_current_dir(new_cwd)?;

                Ok(to.clone())
            }
            _ => Err(Error::store(
                "proc",
                "write",
                format!("Cannot write to proc/self/{}", to[1]),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    #[test]
    fn read_pid() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self/pid")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Integer(pid) => assert_eq!(pid, std::process::id() as i64),
            _ => panic!("Expected integer"),
        }
    }

    #[test]
    fn read_args() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self/args")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Array(args) => assert!(!args.is_empty()),
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn read_cwd() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self/cwd")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert!(!s.is_empty()),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn read_exe() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self/exe")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert!(!s.is_empty()),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn read_env() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self/env")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => assert!(!map.is_empty()),
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_root() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("self"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_self() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("pid"));
                assert!(map.contains_key("cwd"));
                assert!(map.contains_key("args"));
                assert!(map.contains_key("exe"));
                assert!(map.contains_key("env"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let mut store = ProcStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_self_nonexistent_returns_none() {
        let mut store = ProcStore::new();
        let result = store.read(&path!("self/nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_nested_path_returns_none() {
        let mut store = ProcStore::new();
        let result = store.read(&path!("self/pid/extra")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn write_invalid_path_error() {
        let mut store = ProcStore::new();
        let result = store.write(&path!("invalid"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn write_self_pid_error() {
        let mut store = ProcStore::new();
        let result = store.write(&path!("self/pid"), Record::parsed(Value::Integer(123)));
        assert!(result.is_err());
    }

    #[test]
    fn write_cwd_invalid_type_error() {
        let mut store = ProcStore::new();
        let result = store.write(&path!("self/cwd"), Record::parsed(Value::Integer(123)));
        assert!(result.is_err());
    }

    #[test]
    fn write_cwd_nonexistent_error() {
        let mut store = ProcStore::new();
        let result = store.write(
            &path!("self/cwd"),
            Record::parsed(Value::String("/nonexistent/path/12345".to_string())),
        );
        assert!(result.is_err());
    }

    #[test]
    fn default_impl() {
        let store: ProcStore = Default::default();
        assert!(std::ptr::eq(&store as *const _, &store as *const _));
    }
}
