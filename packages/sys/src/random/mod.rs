//! Random number generation store.
//!
//! Provides access to random number generation.
//!
//! ## Paths
//!
//! - `random/u64` - Read returns a random u64
//! - `random/uuid` - Read returns a random UUID v4
//! - `random/bytes` - Write `{"count": N}` returns base64-encoded bytes

use base64::{engine::general_purpose::STANDARD, Engine};
use rand::Rng;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use structfs_store::{Error, Path, Reader, Writer};
use uuid::Uuid;

/// Store for random number generation.
pub struct RandomStore;

impl RandomStore {
    pub fn new() -> Self {
        Self
    }

    fn read_value(&self, path: &Path) -> Result<Option<JsonValue>, Error> {
        if path.is_empty() {
            return Ok(Some(json!({
                "u64": "Random 64-bit unsigned integer",
                "uuid": "Random UUID v4",
                "bytes": "Write {\"count\": N} to get base64-encoded random bytes"
            })));
        }

        if path.components.len() != 1 {
            return Ok(None);
        }

        match path.components[0].as_str() {
            "u64" => {
                let value: u64 = rand::thread_rng().gen();
                Ok(Some(json!(value)))
            }
            "uuid" => {
                let uuid = Uuid::new_v4();
                Ok(Some(JsonValue::String(uuid.to_string())))
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

#[derive(Deserialize)]
struct BytesRequest {
    count: usize,
}

impl Reader for RandomStore {
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
                    message: format!("Failed to deserialize random value: {}", err),
                })?;
            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

impl Writer for RandomStore {
    fn write<RecordType: Serialize>(
        &mut self,
        path: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        if path.components.len() != 1 {
            return Err(Error::ImplementationFailure {
                message: "Invalid random path".to_string(),
            });
        }

        match path.components[0].as_str() {
            "bytes" => {
                let value =
                    serde_json::to_value(data).map_err(|err| Error::RecordSerialization {
                        message: format!("Failed to serialize bytes request: {}", err),
                    })?;

                let request: BytesRequest =
                    serde_json::from_value(value).map_err(|e| Error::ImplementationFailure {
                        message: format!("Invalid bytes request: {}", e),
                    })?;

                if request.count > 1024 * 1024 {
                    return Err(Error::ImplementationFailure {
                        message: "Cannot generate more than 1MB of random bytes".to_string(),
                    });
                }

                let mut bytes = vec![0u8; request.count];
                rand::thread_rng().fill(&mut bytes[..]);

                let encoded = STANDARD.encode(&bytes);

                // Return the base64 string as part of the path
                // Note: This is a workaround since we can't return data from write
                Ok(Path::parse(&encoded).unwrap_or_else(|_| path.clone()))
            }
            _ => Err(Error::ImplementationFailure {
                message: format!("Cannot write to random/{}", path.components[0]),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_u64() {
        let mut store = RandomStore::new();
        let r1: u64 = store
            .read_owned(&Path::parse("u64").unwrap())
            .unwrap()
            .unwrap();
        let r2: u64 = store
            .read_owned(&Path::parse("u64").unwrap())
            .unwrap()
            .unwrap();

        // Both should be valid u64s (just checking they don't panic)
        assert!(r1 != 0 || r2 != 0); // Extremely unlikely both are 0
    }

    #[test]
    fn test_read_uuid() {
        let mut store = RandomStore::new();
        let result: String = store
            .read_owned(&Path::parse("uuid").unwrap())
            .unwrap()
            .unwrap();

        // UUID v4 format: xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx
        assert_eq!(result.len(), 36);
        assert_eq!(&result[14..15], "4"); // Version 4
    }

    #[test]
    fn test_write_bytes() {
        let mut store = RandomStore::new();
        // Just verify the operation succeeds without error
        // Note: The return path mechanism isn't ideal for returning data
        // In a real implementation, we might store the result in a temporary
        // location and return that path instead
        let result = store.write(&Path::parse("bytes").unwrap(), json!({"count": 16}));

        assert!(result.is_ok());
    }
}
