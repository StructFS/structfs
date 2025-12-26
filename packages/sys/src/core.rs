//! New architecture implementations of sys stores using core-store.
//!
//! This module provides implementations using the new three-layer architecture
//! (ll-store, core-store, serde-store) instead of the legacy erased_serde approach.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use structfs_core_store::{
    overlay_store::OverlayStore, Error, NoCodec, Path, Reader, Record, Value, Writer,
};

// ============================================================================
// EnvStore - Environment variables
// ============================================================================

/// Store for environment variable access (new architecture).
pub struct EnvStore;

impl EnvStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            // Return all environment variables as a map
            let vars: BTreeMap<String, Value> = std::env::vars()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            Ok(Some(Value::Map(vars)))
        } else if path.len() == 1 {
            // Return single variable
            let name = &path[0];
            match std::env::var(name) {
                Ok(value) => Ok(Some(Value::String(value))),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => Err(Error::Other {
                    message: "Environment variable contains invalid UTF-8".to_string(),
                }),
            }
        } else {
            Ok(None)
        }
    }
}

impl Default for EnvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for EnvStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for EnvStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.is_empty() {
            return Err(Error::Other {
                message: "Cannot write to root env path".to_string(),
            });
        }

        if to.len() != 1 {
            return Err(Error::Other {
                message: "Nested environment paths not supported".to_string(),
            });
        }

        let name = &to[0];
        let value = data.into_value(&NoCodec)?;

        match value {
            Value::String(s) => {
                std::env::set_var(name, s);
                Ok(to.clone())
            }
            Value::Null => {
                std::env::remove_var(name);
                Ok(to.clone())
            }
            _ => Err(Error::Other {
                message: "Environment variable must be a string or null".to_string(),
            }),
        }
    }
}

// ============================================================================
// TimeStore - Clocks and sleep
// ============================================================================

lazy_static::lazy_static! {
    static ref MONOTONIC_START: Instant = Instant::now();
}

/// Store for time operations (new architecture).
pub struct TimeStore;

impl TimeStore {
    pub fn new() -> Self {
        // Touch the lazy static to initialize it
        let _ = *MONOTONIC_START;
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let mut map = BTreeMap::new();
            map.insert(
                "now".to_string(),
                Value::String("ISO 8601 timestamp".to_string()),
            );
            map.insert(
                "now_unix".to_string(),
                Value::String("Unix timestamp (seconds)".to_string()),
            );
            map.insert(
                "now_unix_ms".to_string(),
                Value::String("Unix timestamp (milliseconds)".to_string()),
            );
            map.insert(
                "monotonic".to_string(),
                Value::String("Monotonic clock (nanoseconds since start)".to_string()),
            );
            map.insert(
                "sleep".to_string(),
                Value::String("Write {\"ms\": N} or {\"secs\": N} to sleep".to_string()),
            );
            return Ok(Some(Value::Map(map)));
        }

        if path.len() != 1 {
            return Ok(None);
        }

        match path[0].as_str() {
            "now" => {
                let now = chrono::Utc::now();
                Ok(Some(Value::String(now.to_rfc3339())))
            }
            "now_unix" => {
                let now = chrono::Utc::now();
                Ok(Some(Value::Integer(now.timestamp())))
            }
            "now_unix_ms" => {
                let now = chrono::Utc::now();
                Ok(Some(Value::Integer(now.timestamp_millis())))
            }
            "monotonic" => {
                let elapsed = MONOTONIC_START.elapsed();
                Ok(Some(Value::Integer(elapsed.as_nanos() as i64)))
            }
            _ => Ok(None),
        }
    }
}

impl Default for TimeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for TimeStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for TimeStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() != 1 {
            return Err(Error::Other {
                message: "Invalid time path".to_string(),
            });
        }

        match to[0].as_str() {
            "sleep" => {
                let value = data.into_value(&NoCodec)?;

                let duration = match &value {
                    Value::Map(map) => {
                        if let Some(Value::Integer(ms)) = map.get("ms") {
                            Duration::from_millis(*ms as u64)
                        } else if let Some(Value::Integer(secs)) = map.get("secs") {
                            Duration::from_secs(*secs as u64)
                        } else {
                            return Err(Error::Other {
                                message: "Sleep requires 'ms' or 'secs' field".to_string(),
                            });
                        }
                    }
                    _ => {
                        return Err(Error::Other {
                            message: "Sleep requires a map with 'ms' or 'secs' field".to_string(),
                        });
                    }
                };

                std::thread::sleep(duration);
                Ok(to.clone())
            }
            _ => Err(Error::Other {
                message: format!("Cannot write to time/{}", to[0]),
            }),
        }
    }
}

// ============================================================================
// RandomStore - Random number generation
// ============================================================================

use rand::Rng;
use uuid::Uuid;

/// Store for random number generation (new architecture).
pub struct RandomStore;

impl RandomStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let mut map = BTreeMap::new();
            map.insert(
                "u64".to_string(),
                Value::String("Random 64-bit unsigned integer".to_string()),
            );
            map.insert(
                "uuid".to_string(),
                Value::String("Random UUID v4".to_string()),
            );
            map.insert(
                "bytes".to_string(),
                Value::String(
                    "Write {\"count\": N} to get base64-encoded random bytes".to_string(),
                ),
            );
            return Ok(Some(Value::Map(map)));
        }

        if path.len() != 1 {
            return Ok(None);
        }

        match path[0].as_str() {
            "u64" => {
                let value: u64 = rand::thread_rng().gen();
                // u64 can exceed i64 max, so we store as string for safety
                Ok(Some(Value::String(value.to_string())))
            }
            "uuid" => {
                let uuid = Uuid::new_v4();
                Ok(Some(Value::String(uuid.to_string())))
            }
            _ => Ok(None),
        }
    }
}

impl Default for RandomStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for RandomStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.read_value(from)?.map(Record::parsed))
    }
}

impl Writer for RandomStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        if to.len() != 1 {
            return Err(Error::Other {
                message: "Invalid random path".to_string(),
            });
        }

        match to[0].as_str() {
            "bytes" => {
                let value = data.into_value(&NoCodec)?;

                let count = match &value {
                    Value::Map(map) => {
                        if let Some(Value::Integer(c)) = map.get("count") {
                            *c as usize
                        } else {
                            return Err(Error::Other {
                                message: "bytes requires 'count' field".to_string(),
                            });
                        }
                    }
                    _ => {
                        return Err(Error::Other {
                            message: "bytes requires a map with 'count' field".to_string(),
                        });
                    }
                };

                if count > 1024 * 1024 {
                    return Err(Error::Other {
                        message: "Cannot generate more than 1MB of random bytes".to_string(),
                    });
                }

                let mut bytes = vec![0u8; count];
                rand::thread_rng().fill(&mut bytes[..]);

                let encoded =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);

                // Return the base64 string as the path (workaround for returning data from write)
                Path::parse(&encoded).map_err(|_| Error::Other {
                    message: "Generated bytes resulted in invalid path".to_string(),
                })
            }
            _ => Err(Error::Other {
                message: format!("Cannot write to random/{}", to[0]),
            }),
        }
    }
}

// ============================================================================
// ProcStore - Process information
// ============================================================================

/// Store for process information (new architecture).
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

// ============================================================================
// DocsStore - Documentation
// ============================================================================

/// Documentation store for sys primitives (new architecture).
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

// ============================================================================
// SysStore - Composite store
// ============================================================================

/// The main system store that composes all OS primitive stores (new architecture).
///
/// Mount this at `/sys` to expose OS functionality through StructFS paths.
pub struct SysStore {
    inner: OverlayStore,
}

impl SysStore {
    /// Create a new system store with all sub-stores mounted.
    pub fn new() -> Self {
        let mut overlay = OverlayStore::new();

        overlay.add_layer(Path::parse("env").unwrap(), Box::new(EnvStore::new()));
        overlay.add_layer(Path::parse("time").unwrap(), Box::new(TimeStore::new()));
        overlay.add_layer(Path::parse("random").unwrap(), Box::new(RandomStore::new()));
        overlay.add_layer(Path::parse("proc").unwrap(), Box::new(ProcStore::new()));
        // Note: FsStore not yet migrated - will be added in a later phase
        overlay.add_layer(Path::parse("docs").unwrap(), Box::new(DocsStore::new()));

        Self { inner: overlay }
    }
}

impl Default for SysStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for SysStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.inner.read(from)
    }
}

impl Writer for SysStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.inner.write(to, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    #[test]
    fn env_read_all() {
        let mut store = EnvStore::new();
        let record = store.read(&path!("")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(_) => {}
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn env_read_var() {
        std::env::set_var("STRUCTFS_CORE_TEST", "test_value");
        let mut store = EnvStore::new();
        let record = store.read(&path!("STRUCTFS_CORE_TEST")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("test_value".to_string()));
        std::env::remove_var("STRUCTFS_CORE_TEST");
    }

    #[test]
    fn env_write_var() {
        let mut store = EnvStore::new();
        let path = path!("STRUCTFS_CORE_WRITE_TEST");
        store
            .write(&path, Record::parsed(Value::String("written".to_string())))
            .unwrap();

        let record = store.read(&path).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("written".to_string()));

        // Cleanup
        store.write(&path, Record::parsed(Value::Null)).unwrap();
    }

    #[test]
    fn time_read_now() {
        let mut store = TimeStore::new();
        let record = store.read(&path!("now")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert!(s.contains("T")),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn time_read_monotonic() {
        let mut store = TimeStore::new();
        let r1 = store.read(&path!("monotonic")).unwrap().unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let r2 = store.read(&path!("monotonic")).unwrap().unwrap();

        let v1 = match r1.into_value(&NoCodec).unwrap() {
            Value::Integer(i) => i,
            _ => panic!("Expected integer"),
        };
        let v2 = match r2.into_value(&NoCodec).unwrap() {
            Value::Integer(i) => i,
            _ => panic!("Expected integer"),
        };
        assert!(v2 > v1);
    }

    #[test]
    fn random_read_uuid() {
        let mut store = RandomStore::new();
        let record = store.read(&path!("uuid")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => {
                assert_eq!(s.len(), 36);
                assert_eq!(&s[14..15], "4"); // Version 4
            }
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn proc_read_pid() {
        let mut store = ProcStore::new();
        let record = store.read(&path!("self/pid")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Integer(pid) => assert_eq!(pid, std::process::id() as i64),
            _ => panic!("Expected integer"),
        }
    }

    #[test]
    fn docs_read_root() {
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
    fn sys_store_read_env() {
        std::env::set_var("STRUCTFS_SYS_TEST", "value");
        let mut store = SysStore::new();
        let record = store
            .read(&path!("env/STRUCTFS_SYS_TEST"))
            .unwrap()
            .unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        assert_eq!(value, Value::String("value".to_string()));
        std::env::remove_var("STRUCTFS_SYS_TEST");
    }

    #[test]
    fn sys_store_read_time() {
        let mut store = SysStore::new();
        let record = store.read(&path!("time/now")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert!(s.contains("T")),
            _ => panic!("Expected string"),
        }
    }
}
