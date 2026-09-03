//! Serde implementations for the core types.
//!
//! - `Path`, `PathComponent`, `Format` serialize as strings.
//! - `Value` serializes structurally (maps to JSON/CBOR/MessagePack shapes).
//! - `Record` serializes as an externally tagged enum:
//!   `{"parsed": <value>}` or `{"raw": {"bytes": [...], "format": "..."}}`.
//!
//! Note: `Value::Bytes` uses serde's byte-string type. Formats without a
//! native byte type (JSON) encode it as an array of numbers, and
//! deserializing that array back yields `Value::Array` — the bytes/array
//! distinction survives only in formats with byte strings (CBOR,
//! MessagePack).

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Format, Path, PathComponent, Record, Value};

impl Serialize for Path {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Path::parse(&s).map_err(de::Error::custom)
    }
}

impl Serialize for PathComponent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PathComponent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        PathComponent::try_new(s).map_err(de::Error::custom)
    }
}

impl Serialize for Format {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Format {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Format::new(s))
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Integer(i) => serializer.serialize_i64(*i),
            Value::Float(f) => serializer.serialize_f64(*f),
            Value::String(s) => serializer.serialize_str(s),
            Value::Bytes(b) => serializer.serialize_bytes(b),
            Value::Array(arr) => {
                let mut seq = serializer.serialize_seq(Some(arr.len()))?;
                for item in arr {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Value::Map(map) => {
                let mut m = serializer.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a StructFS value")
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E>(self, i: i64) -> Result<Value, E> {
        Ok(Value::Integer(i))
    }

    fn visit_u64<E: de::Error>(self, u: u64) -> Result<Value, E> {
        i64::try_from(u)
            .map(Value::Integer)
            .map_err(|_| E::custom(format!("integer {} out of range for i64", u)))
    }

    fn visit_f64<E>(self, f: f64) -> Result<Value, E> {
        Ok(Value::Float(f))
    }

    fn visit_str<E>(self, s: &str) -> Result<Value, E> {
        Ok(Value::String(s.to_string()))
    }

    fn visit_string<E>(self, s: String) -> Result<Value, E> {
        Ok(Value::String(s))
    }

    fn visit_bytes<E>(self, b: &[u8]) -> Result<Value, E> {
        Ok(Value::Bytes(b.to_vec()))
    }

    fn visit_byte_buf<E>(self, b: Vec<u8>) -> Result<Value, E> {
        Ok(Value::Bytes(b))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut arr = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            arr.push(item);
        }
        Ok(Value::Array(arr))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Value, A::Error> {
        let mut map = BTreeMap::new();
        while let Some((k, v)) = access.next_entry::<String, Value>()? {
            map.insert(k, v);
        }
        Ok(Value::Map(map))
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// Wire representation of `Record`. Kept separate from `Record` itself so
/// the in-memory type can hold `Bytes` and stay `#[non_exhaustive]`.
#[derive(Serialize, Deserialize)]
enum RecordRepr {
    #[serde(rename = "raw")]
    Raw { bytes: Vec<u8>, format: Format },
    #[serde(rename = "parsed")]
    Parsed(Value),
}

impl Serialize for Record {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let repr = match self {
            Record::Raw { bytes, format } => RecordRepr::Raw {
                bytes: bytes.to_vec(),
                format: format.clone(),
            },
            Record::Parsed(v) => RecordRepr::Parsed(v.clone()),
        };
        repr.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Record {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match RecordRepr::deserialize(deserializer)? {
            RecordRepr::Raw { bytes, format } => Record::raw(bytes, format),
            RecordRepr::Parsed(v) => Record::Parsed(v),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

    #[test]
    fn path_roundtrip() {
        let p = path!("users/123/name");
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"users/123/name\"");
        let back: Path = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn empty_path_roundtrip() {
        let p = path!();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"\"");
        let back: Path = serde_json::from_str(&json).unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn invalid_path_rejected() {
        let result: Result<Path, _> = serde_json::from_str("\"bad-component\"");
        assert!(result.is_err());
    }

    #[test]
    fn path_component_roundtrip() {
        let c = PathComponent::try_new("alice").unwrap();
        let json = serde_json::to_string(&c).unwrap();
        let back: PathComponent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);

        let result: Result<PathComponent, _> = serde_json::from_str("\"bad name\"");
        assert!(result.is_err());
    }

    #[test]
    fn format_roundtrip() {
        let json = serde_json::to_string(&Format::JSON).unwrap();
        assert_eq!(json, "\"application/json\"");
        let back: Format = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Format::JSON);
    }

    #[test]
    fn value_roundtrip_via_json() {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Value::from("Alice"));
        map.insert("age".to_string(), Value::from(30i64));
        map.insert("score".to_string(), Value::Float(0.5));
        map.insert("active".to_string(), Value::Bool(true));
        map.insert("nothing".to_string(), Value::Null);
        map.insert(
            "tags".to_string(),
            Value::Array(vec![Value::from("a"), Value::from("b")]),
        );
        let value = Value::Map(map);

        let json = serde_json::to_string(&value).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn value_interops_with_serde_json() {
        // A Value serializes to the JSON shape you'd expect
        let value = Value::Map({
            let mut m = BTreeMap::new();
            m.insert("x".to_string(), Value::Integer(1));
            m
        });
        let json: serde_json::Value = serde_json::to_value(&value).unwrap();
        assert_eq!(json, serde_json::json!({"x": 1}));
    }

    #[test]
    fn value_bytes_become_arrays_in_json() {
        // JSON has no byte type: bytes serialize as arrays and come back as
        // arrays. Documented limitation of self-describing formats.
        let value = Value::Bytes(vec![1, 2, 3]);
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, "[1,2,3]");
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back,
            Value::Array(vec![
                Value::Integer(1),
                Value::Integer(2),
                Value::Integer(3)
            ])
        );
    }

    #[test]
    fn u64_overflow_rejected() {
        let result: Result<Value, _> = serde_json::from_str("18446744073709551615");
        assert!(result.is_err());
    }

    #[test]
    fn record_parsed_roundtrip() {
        let record = Record::parsed(Value::from("hello"));
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(json, "{\"parsed\":\"hello\"}");
        let back: Record = serde_json::from_str(&json).unwrap();
        assert!(back.is_parsed());
        assert_eq!(back.as_value(), Some(&Value::from("hello")));
    }

    #[test]
    fn record_raw_roundtrip() {
        let record = Record::raw(bytes::Bytes::from_static(b"{}"), Format::JSON);
        let json = serde_json::to_string(&record).unwrap();
        let back: Record = serde_json::from_str(&json).unwrap();
        assert!(back.is_raw());
        assert_eq!(back.format(), Format::JSON);
        assert_eq!(back.as_bytes().map(|b| b.as_ref()), Some(b"{}".as_ref()));
    }
}
