//! Process information store.

use std::collections::BTreeMap;

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

/// Store for process information.
pub struct ProcStore;

impl ProcStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let mut self_map = BTreeMap::new();
            self_map.insert(
                "pid".to_string(),
                Value::String("Current process ID".to_string()),
            );
            self_map.insert(
                "cwd".to_string(),
                Value::String("Current working directory".to_string()),
            );
            self_map.insert(
                "args".to_string(),
                Value::String("Command line arguments".to_string()),
            );
            self_map.insert(
                "exe".to_string(),
                Value::String("Path to current executable".to_string()),
            );
            self_map.insert(
                "env".to_string(),
                Value::String("Environment variables".to_string()),
            );

            let mut map = BTreeMap::new();
            map.insert("self".to_string(), Value::Map(self_map));
            return Ok(Some(Value::Map(map)));
        }

        // Must start with "self"
        if path[0].as_str() != "self" {
            return Ok(None);
        }

        if path.len() == 1 {
            // Just /proc/self - list available info
            let mut map = BTreeMap::new();
            map.insert(
                "pid".to_string(),
                Value::String("Current process ID".to_string()),
            );
            map.insert(
                "cwd".to_string(),
                Value::String("Current working directory".to_string()),
            );
            map.insert(
                "args".to_string(),
                Value::String("Command line arguments".to_string()),
            );
            map.insert(
                "exe".to_string(),
                Value::String("Path to current executable".to_string()),
            );
            map.insert(
                "env".to_string(),
                Value::String("Environment variables".to_string()),
            );
            return Ok(Some(Value::Map(map)));
        }

        if path.len() != 2 {
            return Ok(None);
        }

        match path[1].as_str() {
            "pid" => Ok(Some(Value::Integer(std::process::id() as i64))),
            "cwd" => match std::env::current_dir() {
                Ok(cwd) => Ok(Some(Value::String(cwd.to_string_lossy().to_string()))),
                Err(e) => Err(Error::Other {
                    message: format!("Failed to get cwd: {}", e),
                }),
            },
            "args" => {
                let args: Vec<Value> = std::env::args().map(Value::String).collect();
                Ok(Some(Value::Array(args)))
            }
            "exe" => match std::env::current_exe() {
                Ok(exe) => Ok(Some(Value::String(exe.to_string_lossy().to_string()))),
                Err(e) => Err(Error::Other {
                    message: format!("Failed to get exe: {}", e),
                }),
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
            return Err(Error::Other {
                message: "Invalid proc path".to_string(),
            });
        }

        match to[1].as_str() {
            "cwd" => {
                let value = data.into_value(&NoCodec)?;

                let new_cwd = match &value {
                    Value::String(s) => s.as_str(),
                    _ => {
                        return Err(Error::Other {
                            message: "cwd must be a string path".to_string(),
                        });
                    }
                };

                std::env::set_current_dir(new_cwd).map_err(|e| Error::Other {
                    message: format!("Failed to change directory: {}", e),
                })?;

                Ok(to.clone())
            }
            _ => Err(Error::Other {
                message: format!("Cannot write to proc/self/{}", to[1]),
            }),
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
