//! Time store.
//!
//! Provides access to system clocks and sleep functionality.
//!
//! ## Paths
//!
//! - `time/now` - Read returns current time as ISO 8601 string
//! - `time/now_unix` - Read returns Unix timestamp in seconds
//! - `time/now_unix_ms` - Read returns Unix timestamp in milliseconds
//! - `time/monotonic` - Read returns monotonic clock value (nanoseconds)
//! - `time/sleep` - Write `{"ms": N}` or `{"secs": N}` to sleep

use chrono::Utc;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::time::{Duration, Instant};
use structfs_store::{Error, Path, Reader, Writer};

// Use a lazy static to get a consistent monotonic reference point
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

    fn read_value(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.is_empty() {
            return Ok(Some(json!({
                "now": "ISO 8601 timestamp",
                "now_unix": "Unix timestamp (seconds)",
                "now_unix_ms": "Unix timestamp (milliseconds)",
                "monotonic": "Monotonic clock (nanoseconds since start)",
                "sleep": "Write {\"ms\": N} or {\"secs\": N} to sleep"
            })));
        }

        if path.components.len() != 1 {
            return Ok(None);
        }

        match path.components[0].as_str() {
            "now" => {
                let now = Utc::now();
                Ok(Some(JsonValue::String(now.to_rfc3339())))
            }
            "now_unix" => {
                let now = Utc::now();
                Ok(Some(json!(now.timestamp())))
            }
            "now_unix_ms" => {
                let now = Utc::now();
                Ok(Some(json!(now.timestamp_millis())))
            }
            "monotonic" => {
                let elapsed = MONOTONIC_START.elapsed();
                Ok(Some(json!(elapsed.as_nanos() as u64)))
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

#[derive(Deserialize)]
struct SleepRequest {
    #[serde(default)]
    ms: Option<u64>,
    #[serde(default)]
    secs: Option<u64>,
}

impl Reader for TimeStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
        Ok(self.read_value(from)?.map(|v| {
            let de: Box<dyn erased_serde::Deserializer> =
                Box::new(<dyn erased_serde::Deserializer>::erase(v));
            de
        }))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error> {
        if let Some(value) = self.read_value(from)? {
            let data: RecordType =
                serde_json::from_value(value).map_err(|err| Error::RecordDeserialization {
                    message: format!("Failed to deserialize time value: {}", err),
                })?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

impl Writer for TimeStore {
    fn write<RecordType: Serialize>(
        &mut self,
        path: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        if path.components.len() != 1 {
            return Err(Error::ImplementationFailure {
                message: "Invalid time path".to_string(),
            });
        }

        match path.components[0].as_str() {
            "sleep" => {
                let value =
                    serde_json::to_value(data).map_err(|err| Error::RecordSerialization {
                        message: format!("Failed to serialize sleep request: {}", err),
                    })?;

                let request: SleepRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid sleep request: {}", e),
                    })?;

                let duration = match (request.ms, request.secs) {
                    (Some(ms), _) => Duration::from_millis(ms),
                    (None, Some(secs)) => Duration::from_secs(secs),
                    (None, None) => {
                        return Err(Error::ImplementationFailure {
                            message: "Sleep requires 'ms' or 'secs' field".to_string(),
                        })
                    }
                };

                std::thread::sleep(duration);
                Ok(path.clone())
            }
            _ => Err(Error::ImplementationFailure {
                message: format!("Cannot write to time/{}", path.components[0]),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_now() {
        let mut store = TimeStore::new();
        let result: Option<String> = store.read_owned(&Path::parse("now").unwrap()).unwrap();
        assert!(result.is_some());
        let s = result.unwrap();
        // Should be valid ISO 8601
        assert!(s.contains("T"));
    }

    #[test]
    fn test_read_now_unix() {
        let mut store = TimeStore::new();
        let result: Option<i64> = store.read_owned(&Path::parse("now_unix").unwrap()).unwrap();
        assert!(result.is_some());
        let ts = result.unwrap();
        // Should be a reasonable timestamp (after 2020)
        assert!(ts > 1577836800);
    }

    #[test]
    fn test_read_monotonic() {
        let mut store = TimeStore::new();
        let r1: u64 = store
            .read_owned(&Path::parse("monotonic").unwrap())
            .unwrap()
            .unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let r2: u64 = store
            .read_owned(&Path::parse("monotonic").unwrap())
            .unwrap()
            .unwrap();
        assert!(r2 > r1);
    }

    #[test]
    fn test_sleep() {
        let mut store = TimeStore::new();
        let start = std::time::Instant::now();

        store
            .write(&Path::parse("sleep").unwrap(), json!({"ms": 50}))
            .unwrap();

        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
    }
}
