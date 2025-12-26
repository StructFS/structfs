//! Time and clock store.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use structfs_core_store::{Error, NoCodec, Path, Reader, Record, Value, Writer};

lazy_static::lazy_static! {
    static ref MONOTONIC_START: Instant = Instant::now();
}

/// Store for time operations.
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
            return Err(Error::store("time", "write", "Invalid time path"));
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
                            return Err(Error::store(
                                "time",
                                "sleep",
                                "Sleep requires 'ms' or 'secs' field",
                            ));
                        }
                    }
                    _ => {
                        return Err(Error::store(
                            "time",
                            "sleep",
                            "Sleep requires a map with 'ms' or 'secs' field",
                        ));
                    }
                };

                std::thread::sleep(duration);
                Ok(to.clone())
            }
            _ => Err(Error::store(
                "time",
                "write",
                format!("Cannot write to time/{}", to[0]),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::Duration;
    use structfs_core_store::path;

    #[test]
    fn read_now() {
        let mut store = TimeStore::new();
        let record = store.read(&path!("now")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::String(s) => assert!(s.contains("T")),
            _ => panic!("Expected string"),
        }
    }

    #[test]
    fn read_now_unix() {
        let mut store = TimeStore::new();
        let record = store.read(&path!("now_unix")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Integer(ts) => assert!(ts > 0),
            _ => panic!("Expected integer"),
        }
    }

    #[test]
    fn read_now_unix_ms() {
        let mut store = TimeStore::new();
        let record = store.read(&path!("now_unix_ms")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Integer(ts) => assert!(ts > 0),
            _ => panic!("Expected integer"),
        }
    }

    #[test]
    fn read_monotonic() {
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
    fn read_root() {
        let mut store = TimeStore::new();
        let record = store.read(&path!("")).unwrap().unwrap();
        let value = record.into_value(&NoCodec).unwrap();
        match value {
            Value::Map(map) => {
                assert!(map.contains_key("now"));
                assert!(map.contains_key("now_unix"));
                assert!(map.contains_key("monotonic"));
                assert!(map.contains_key("sleep"));
            }
            _ => panic!("Expected map"),
        }
    }

    #[test]
    fn read_nonexistent() {
        let mut store = TimeStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_nested_path_returns_none() {
        let mut store = TimeStore::new();
        let result = store.read(&path!("now/extra")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn write_sleep_ms() {
        let mut store = TimeStore::new();
        let mut map = BTreeMap::new();
        map.insert("ms".to_string(), Value::Integer(1));

        let before = std::time::Instant::now();
        store
            .write(&path!("sleep"), Record::parsed(Value::Map(map)))
            .unwrap();
        let elapsed = before.elapsed();

        assert!(elapsed.as_millis() >= 1);
    }

    #[test]
    fn write_sleep_secs() {
        let mut store = TimeStore::new();
        let mut map = BTreeMap::new();
        map.insert("secs".to_string(), Value::Integer(0));

        // Just test that it doesn't error
        store
            .write(&path!("sleep"), Record::parsed(Value::Map(map)))
            .unwrap();
    }

    #[test]
    fn write_sleep_missing_field_error() {
        let mut store = TimeStore::new();
        let mut map = BTreeMap::new();
        map.insert("invalid".to_string(), Value::Integer(100));

        let result = store.write(&path!("sleep"), Record::parsed(Value::Map(map)));
        assert!(result.is_err());
    }

    #[test]
    fn write_sleep_invalid_type_error() {
        let mut store = TimeStore::new();
        let result = store.write(
            &path!("sleep"),
            Record::parsed(Value::String("100".to_string())),
        );
        assert!(result.is_err());
    }

    #[test]
    fn write_to_now_error() {
        let mut store = TimeStore::new();
        let result = store.write(&path!("now"), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn write_invalid_path_length_error() {
        let mut store = TimeStore::new();
        let result = store.write(&path!(""), Record::parsed(Value::Null));
        assert!(result.is_err());
    }

    #[test]
    fn default_impl() {
        let store: TimeStore = Default::default();
        assert!(std::ptr::eq(&store as *const _, &store as *const _)); // Just verify it works
    }
}
