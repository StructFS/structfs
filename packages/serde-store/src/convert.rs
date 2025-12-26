//! Conversions between Value and serde types.

use serde::de::DeserializeOwned;
use serde::Serialize;
use structfs_core_store::{Error, Value};

/// Convert a Value to a Rust type via serde.
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    // Convert Value to serde_json::Value first, then deserialize
    let json = value_to_json(value);
    serde_json::from_value(json)
        .map_err(|e| Error::decode(structfs_core_store::Format::VALUE, e.to_string()))
}

/// Convert a Rust type to a Value via serde.
pub fn to_value<T: Serialize>(data: &T) -> Result<Value, Error> {
    // Serialize to serde_json::Value first, then convert to Value
    let json = serde_json::to_value(data)
        .map_err(|e| Error::encode(structfs_core_store::Format::VALUE, e.to_string()))?;
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

    #[test]
    fn value_to_json_null() {
        let value = Value::Null;
        let json = value_to_json(value);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn value_to_json_bool() {
        assert_eq!(
            value_to_json(Value::Bool(true)),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            value_to_json(Value::Bool(false)),
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn value_to_json_string() {
        let value = Value::String("hello world".to_string());
        let json = value_to_json(value);
        assert_eq!(json, serde_json::Value::String("hello world".to_string()));
    }

    #[test]
    fn value_to_json_integer() {
        let value = Value::Integer(12345);
        let json = value_to_json(value);
        assert_eq!(json, serde_json::json!(12345));
    }

    #[test]
    fn value_to_json_float() {
        let value = Value::Float(1.23456);
        let json = value_to_json(value);
        if let serde_json::Value::Number(n) = json {
            assert!((n.as_f64().unwrap() - 1.23456).abs() < 0.00001);
        } else {
            panic!("expected number");
        }
    }

    #[test]
    fn value_to_json_nan_becomes_null() {
        let value = Value::Float(f64::NAN);
        let json = value_to_json(value);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn value_to_json_bytes() {
        let value = Value::Bytes(vec![1, 2, 3, 4]);
        let json = value_to_json(value);

        // Should be base64 encoded
        if let serde_json::Value::String(s) = json {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&s)
                .unwrap();
            assert_eq!(decoded, vec![1, 2, 3, 4]);
        } else {
            panic!("expected string");
        }
    }

    #[test]
    fn value_to_json_map() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("key".to_string(), Value::String("value".to_string()));
        map.insert("num".to_string(), Value::Integer(42));

        let json = value_to_json(Value::Map(map));
        assert_eq!(json, serde_json::json!({"key": "value", "num": 42}));
    }

    #[test]
    fn json_to_value_null() {
        let json = serde_json::Value::Null;
        let value = json_to_value(json);
        assert_eq!(value, Value::Null);
    }

    #[test]
    fn json_to_value_bool() {
        assert_eq!(
            json_to_value(serde_json::Value::Bool(true)),
            Value::Bool(true)
        );
        assert_eq!(
            json_to_value(serde_json::Value::Bool(false)),
            Value::Bool(false)
        );
    }

    #[test]
    fn json_to_value_string() {
        let json = serde_json::Value::String("test".to_string());
        let value = json_to_value(json);
        assert_eq!(value, Value::String("test".to_string()));
    }

    #[test]
    fn json_to_value_array() {
        let json = serde_json::json!([1, "two", true]);
        let value = json_to_value(json);
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], Value::Integer(1));
                assert_eq!(arr[1], Value::String("two".to_string()));
                assert_eq!(arr[2], Value::Bool(true));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn json_to_value_object() {
        let json = serde_json::json!({"a": 1, "b": "two"});
        let value = json_to_value(json);
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("a"), Some(&Value::Integer(1)));
                assert_eq!(map.get("b"), Some(&Value::String("two".to_string())));
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn from_value_error() {
        // Try to deserialize a string into a struct
        let value = Value::String("not a struct".to_string());
        let result: Result<TestStruct, _> = from_value(value);
        assert!(result.is_err());
    }

    #[test]
    fn to_value_primitives() {
        assert_eq!(to_value(&42i32).unwrap(), Value::Integer(42));
        assert_eq!(
            to_value(&"hello").unwrap(),
            Value::String("hello".to_string())
        );
        assert_eq!(to_value(&true).unwrap(), Value::Bool(true));
    }

    #[test]
    fn to_value_vec() {
        let vec = vec![1, 2, 3];
        let value = to_value(&vec).unwrap();
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], Value::Integer(1));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn roundtrip_nested_struct() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Inner {
            value: i32,
        }

        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Outer {
            inner: Inner,
            items: Vec<String>,
        }

        let original = Outer {
            inner: Inner { value: 99 },
            items: vec!["a".to_string(), "b".to_string()],
        };

        let value = to_value(&original).unwrap();
        let recovered: Outer = from_value(value).unwrap();
        assert_eq!(original, recovered);
    }

    #[test]
    fn roundtrip_option() {
        let some_value: Option<i32> = Some(42);
        let none_value: Option<i32> = None;

        let some_converted = to_value(&some_value).unwrap();
        let none_converted = to_value(&none_value).unwrap();

        let some_recovered: Option<i32> = from_value(some_converted).unwrap();
        let none_recovered: Option<i32> = from_value(none_converted).unwrap();

        assert_eq!(some_recovered, Some(42));
        assert_eq!(none_recovered, None);
    }
}
