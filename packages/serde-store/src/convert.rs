//! Checked conversion to/from a plain JSON DOM. Original token spellings and
//! duplicate keys are already lost in a DOM; use JsonCodec on untrusted wire input.
use crate::limits::DEPTH_CEILING;
pub use crate::value_serde::{from_value, to_value};
use crate::Limits;
use structfs_core_store::{CodecErrorKind, CodecOperation, Error, Format, Value};

fn json_error(kind: CodecErrorKind, message: String) -> Error {
    Error::Codec {
        kind,
        operation: CodecOperation::Encode,
        format: Format::JSON,
        message,
    }
}

fn unsupported(what: &str) -> Error {
    json_error(
        CodecErrorKind::UnsupportedValue,
        format!("plain JSON cannot represent {what}"),
    )
}

/// Convert to plain JSON, rejecting bytes and non-finite floats.
///
/// This walks the two DOMs directly rather than encoding to bytes and
/// reparsing: the answer is the same and the cost is a fraction of it.
/// Nesting is still bounded by [`Limits::default`]'s `max_depth`, since the
/// walk recurses on the native stack.
pub fn value_to_json(value: Value) -> Result<serde_json::Value, Error> {
    let max_depth = Limits::default().max_depth.min(DEPTH_CEILING);
    value_to_json_at(value, 0, max_depth)
}

fn value_to_json_at(
    value: Value,
    depth: usize,
    max_depth: usize,
) -> Result<serde_json::Value, Error> {
    if depth > max_depth {
        return Err(json_error(
            CodecErrorKind::ResourceLimit,
            format!("value nests deeper than {max_depth} levels"),
        ));
    }
    Ok(match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(v) => serde_json::Value::Bool(v),
        Value::Integer(v) => serde_json::Value::Number(v.into()),
        Value::Unsigned(v) => serde_json::Value::Number(v.into()),
        Value::Float(v) => serde_json::Number::from_f64(v)
            .map(serde_json::Value::Number)
            .ok_or_else(|| unsupported("a non-finite float"))?,
        Value::String(v) => serde_json::Value::String(v),
        Value::Bytes(_) => return Err(unsupported("a byte string")),
        Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(|v| value_to_json_at(v, depth + 1, max_depth))
                .collect::<Result<_, _>>()?,
        ),
        Value::Map(entries) => serde_json::Value::Object(
            entries
                .into_iter()
                .map(|(k, v)| value_to_json_at(v, depth + 1, max_depth).map(|v| (k, v)))
                .collect::<Result<_, _>>()?,
        ),
        other => return Err(unsupported(&format!("{other:?}"))),
    })
}

/// Import an already parsed JSON DOM, preserving its exact supported numbers.
pub fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(v) => Value::Bool(v),
        serde_json::Value::String(v) => Value::String(v),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(u) = n.as_u64() {
                Value::from(u)
            } else if let Some(f) = n.as_f64() {
                Value::from(f)
            } else {
                // Only reachable if serde_json's `arbitrary_precision` feature
                // is turned on somewhere in the dependency graph, where a
                // Number holds an unparsed literal that is none of the three
                // (e.g. `1e400`). Use the nearest f64 only if it is finite —
                // an overflow to infinity would invent a value the document
                // never held — and otherwise keep the literal's digits as a
                // string rather than panicking.
                let literal = n.to_string();
                match literal.parse::<f64>() {
                    Ok(f) if f.is_finite() => Value::from(f),
                    _ => Value::String(literal),
                }
            }
        }
        serde_json::Value::Array(a) => Value::Array(a.into_iter().map(json_to_value).collect()),
        serde_json::Value::Object(m) => {
            Value::Map(m.into_iter().map(|(k, v)| (k, json_to_value(v))).collect())
        }
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

    /// Pairs that must convert to each other in both directions. One table
    /// replaces a dozen one-assertion tests that each covered a single
    /// variant.
    fn equivalent_pairs() -> Vec<(Value, serde_json::Value)> {
        use std::collections::BTreeMap;
        vec![
            (Value::Null, serde_json::Value::Null),
            (Value::Bool(true), serde_json::json!(true)),
            (Value::Bool(false), serde_json::json!(false)),
            (Value::Integer(12345), serde_json::json!(12345)),
            (Value::Integer(-100), serde_json::json!(-100)),
            (Value::Integer(0), serde_json::json!(0)),
            (
                Value::String("hello world".to_string()),
                serde_json::json!("hello world"),
            ),
            (Value::String(String::new()), serde_json::json!("")),
            (Value::Array(vec![]), serde_json::json!([])),
            (
                Value::Array(vec![
                    Value::Integer(1),
                    Value::Integer(2),
                    Value::Integer(3),
                ]),
                serde_json::json!([1, 2, 3]),
            ),
            (
                Value::Array(vec![
                    Value::Integer(1),
                    Value::String("two".to_string()),
                    Value::Bool(true),
                ]),
                serde_json::json!([1, "two", true]),
            ),
            (Value::Map(BTreeMap::new()), serde_json::json!({})),
            (
                Value::Map(
                    [
                        ("key".to_string(), Value::String("value".to_string())),
                        ("num".to_string(), Value::Integer(42)),
                    ]
                    .into_iter()
                    .collect(),
                ),
                serde_json::json!({"key": "value", "num": 42}),
            ),
            (
                Value::Map(
                    [(
                        "nested".to_string(),
                        Value::Array(vec![Value::Map(
                            [("deep".to_string(), Value::Null)].into_iter().collect(),
                        )]),
                    )]
                    .into_iter()
                    .collect(),
                ),
                serde_json::json!({"nested": [{"deep": null}]}),
            ),
        ]
    }

    #[test]
    fn value_and_json_doms_agree() {
        for (value, json) in equivalent_pairs() {
            assert_eq!(
                value_to_json(value.clone()).unwrap(),
                json,
                "value_to_json({value:?})"
            );
            assert_eq!(json_to_value(json.clone()), value, "json_to_value({json})");
        }
    }

    #[test]
    fn floats_convert_approximately() {
        for f in [1.23456f64, 2.75, -0.5, 0.0, 1e300] {
            match value_to_json(Value::Float(f)).unwrap() {
                serde_json::Value::Number(n) => {
                    assert!((n.as_f64().unwrap() - f).abs() <= f.abs() * 1e-12)
                }
                other => panic!("expected number, got {other}"),
            }
        }
        assert_eq!(json_to_value(serde_json::json!(2.75)), Value::Float(2.75));
    }

    #[test]
    fn value_to_json_rejects_lossy_values() {
        assert!(value_to_json(Value::Float(f64::NAN)).is_err());
        assert!(value_to_json(Value::Float(f64::INFINITY)).is_err());
        assert!(value_to_json(Value::Bytes(vec![1, 2, 3])).is_err());
    }

    #[test]
    fn value_to_json_bounds_nesting() {
        let mut deep = Value::Null;
        for _ in 0..200 {
            deep = Value::Array(vec![deep]);
        }
        assert!(value_to_json(deep).is_err());
    }

    #[test]
    fn unsigned_beyond_i64_survives() {
        let json = value_to_json(Value::Unsigned(u64::MAX)).unwrap();
        assert_eq!(json, serde_json::json!(u64::MAX));
        assert_eq!(json_to_value(json), Value::Unsigned(u64::MAX));
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
