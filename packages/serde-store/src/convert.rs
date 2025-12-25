//! Conversions between Value and serde types.

use serde::de::DeserializeOwned;
use serde::Serialize;
use structfs_core_store::{Error, Value};

/// Convert a Value to a Rust type via serde.
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    // Convert Value to serde_json::Value first, then deserialize
    let json = value_to_json(value);
    serde_json::from_value(json).map_err(|e| Error::Decode {
        format: structfs_core_store::Format::VALUE,
        message: e.to_string(),
    })
}

/// Convert a Rust type to a Value via serde.
pub fn to_value<T: Serialize>(data: &T) -> Result<Value, Error> {
    // Serialize to serde_json::Value first, then convert to Value
    let json = serde_json::to_value(data).map_err(|e| Error::Encode {
        format: structfs_core_store::Format::VALUE,
        message: e.to_string(),
    })?;
    Ok(json_to_value(json))
}

/// Convert our Value to serde_json::Value.
pub fn value_to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        Value::Integer(i) => serde_json::Value::Number(i.into()),
        Value::Float(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s),
        Value::Bytes(b) => {
            // JSON doesn't have bytes, so we base64 encode
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

/// Convert serde_json::Value to our Value.
pub fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                // Fallback for very large numbers
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestStruct {
        name: String,
        age: u32,
        active: bool,
    }

    #[test]
    fn roundtrip_struct() {
        let original = TestStruct {
            name: "Alice".to_string(),
            age: 30,
            active: true,
        };

        let value = to_value(&original).unwrap();
        let recovered: TestStruct = from_value(value).unwrap();

        assert_eq!(original, recovered);
    }

    #[test]
    fn json_to_value_numbers() {
        let json = serde_json::json!({
            "integer": 42,
            "float": 2.75,
            "negative": -100
        });

        let value = json_to_value(json);
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("integer"), Some(&Value::Integer(42)));
                assert_eq!(map.get("negative"), Some(&Value::Integer(-100)));
                // Float comparison
                if let Some(Value::Float(f)) = map.get("float") {
                    assert!((f - 2.75).abs() < 0.001);
                } else {
                    panic!("expected float");
                }
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn value_to_json_arrays() {
        let value = Value::Array(vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ]);

        let json = value_to_json(value);
        assert_eq!(json, serde_json::json!([1, 2, 3]));
    }
}
