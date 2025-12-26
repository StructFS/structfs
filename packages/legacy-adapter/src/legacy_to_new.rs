//! Adapter to wrap legacy stores for use with new core-store traits.

use structfs_core_store::{Error as CoreError, Path as CorePath, Record, Value};
use structfs_store::{Reader as LegacyReader, Writer as LegacyWriter};

use crate::path_convert::{core_path_to_legacy, legacy_path_to_core};
use crate::Error;

/// Wraps a legacy store to implement new core-store traits.
///
/// This adapter allows legacy stores (implementing `structfs_store::Reader` and
/// `structfs_store::Writer`) to be used with the new `structfs_core_store` traits.
///
/// # Example
///
/// ```rust,ignore
/// use structfs_legacy_adapter::LegacyToNew;
/// use structfs_core_store::{Reader, path};
///
/// let legacy_store = MyLegacyStore::new();
/// let mut new_store = LegacyToNew::new(legacy_store);
///
/// // Now use with new core-store API
/// let record = new_store.read(&path!("foo/bar"))?;
/// ```
pub struct LegacyToNew<S> {
    inner: S,
}

impl<S> LegacyToNew<S> {
    /// Create a new adapter wrapping the given legacy store.
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Get a reference to the inner store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Get a mutable reference to the inner store.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Unwrap and return the inner store.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: LegacyReader + Send + Sync> structfs_core_store::Reader for LegacyToNew<S> {
    fn read(&mut self, from: &CorePath) -> Result<Option<Record>, CoreError> {
        // Convert path
        let legacy_path = core_path_to_legacy(from).map_err(CoreError::from)?;

        // Read as serde_json::Value from the legacy store
        let maybe_json: Option<serde_json::Value> = self
            .inner
            .read_owned(&legacy_path)
            .map_err(Error::LegacyStore)
            .map_err(CoreError::from)?;

        match maybe_json {
            Some(json) => {
                // Convert serde_json::Value to core_store::Value
                let value = json_to_value(json);
                Ok(Some(Record::parsed(value)))
            }
            None => Ok(None),
        }
    }
}

impl<S: LegacyWriter + Send + Sync> structfs_core_store::Writer for LegacyToNew<S> {
    fn write(&mut self, to: &CorePath, data: Record) -> Result<CorePath, CoreError> {
        // Convert path
        let legacy_path = core_path_to_legacy(to).map_err(CoreError::from)?;

        // Get Value from Record (use NoCodec since we're going to JSON anyway)
        let value = data
            .into_value(&structfs_core_store::NoCodec)
            .map_err(|e| {
                // If we can't get the value without a codec, try to decode the raw bytes as JSON
                CoreError::Other {
                    message: format!("Cannot convert record to value: {}", e),
                }
            })?;

        // Convert to serde_json::Value
        let json = value_to_json(value);

        // Write using legacy API
        let result_path = self
            .inner
            .write(&legacy_path, &json)
            .map_err(Error::LegacyStore)
            .map_err(CoreError::from)?;

        // Convert result path back
        legacy_path_to_core(&result_path).map_err(CoreError::from)
    }
}

/// Convert serde_json::Value to core_store::Value.
fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                // Very large numbers - store as string
                Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(arr) => Value::Array(arr.into_iter().map(json_to_value).collect()),
        serde_json::Value::Object(map) => Value::Map(
            map.into_iter()
                .map(|(k, v)| (k, json_to_value(v)))
                .collect(),
        ),
    }
}

/// Convert core_store::Value to serde_json::Value.
fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        Value::Integer(i) => serde_json::Value::Number(i.into()),
        Value::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s),
        Value::Bytes(b) => {
            // JSON doesn't have bytes, so base64 encode
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&b);
            serde_json::Value::String(encoded)
        }
        Value::Array(arr) => serde_json::Value::Array(arr.into_iter().map(value_to_json).collect()),
        Value::Map(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, value_to_json(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_to_value_primitives() {
        assert_eq!(json_to_value(serde_json::json!(null)), Value::Null);
        assert_eq!(json_to_value(serde_json::json!(true)), Value::Bool(true));
        assert_eq!(json_to_value(serde_json::json!(42)), Value::Integer(42));
        assert_eq!(
            json_to_value(serde_json::json!("hello")),
            Value::String("hello".to_string())
        );
    }

    #[test]
    fn json_to_value_complex() {
        let json = serde_json::json!({
            "name": "Alice",
            "age": 30,
            "active": true,
            "scores": [1, 2, 3]
        });

        let value = json_to_value(json);
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("name"), Some(&Value::String("Alice".to_string())));
                assert_eq!(map.get("age"), Some(&Value::Integer(30)));
                assert_eq!(map.get("active"), Some(&Value::Bool(true)));
                match map.get("scores") {
                    Some(Value::Array(arr)) => {
                        assert_eq!(arr.len(), 3);
                    }
                    _ => panic!("expected array"),
                }
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn value_to_json_roundtrips() {
        let original = serde_json::json!({
            "string": "hello",
            "number": 42,
            "float": 2.75,
            "bool": true,
            "null": null,
            "array": [1, 2, 3],
            "object": {"nested": "value"}
        });

        let value = json_to_value(original.clone());
        let back = value_to_json(value);

        assert_eq!(original, back);
    }
}
