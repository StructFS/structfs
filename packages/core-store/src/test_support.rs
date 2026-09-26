//! Shared test helpers: a JSON codec over `serde_json`, and a raw-capable
//! map store for tests that need to hold `Record::Raw` verbatim (which
//! `MemoryStore` deliberately cannot).

use std::collections::{BTreeMap, HashMap};

use bytes::Bytes;

use crate::{Codec, Error, Format, Path, Reader, Record, Value, Writer};

/// Test codec that parses JSON using serde_json (dev-dependency).
pub(crate) struct TestJsonCodec;

impl Codec for TestJsonCodec {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        if format != &Format::JSON {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        let json: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| Error::decode(format.clone(), e.to_string()))?;
        Ok(json_to_value(json))
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        if format != &Format::JSON {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        let json = value_to_json(value);
        let bytes =
            serde_json::to_vec(&json).map_err(|e| Error::encode(format.clone(), e.to_string()))?;
        Ok(Bytes::from(bytes))
    }

    fn supports(&self, format: &Format) -> bool {
        format == &Format::JSON
    }
}

pub(crate) fn json_to_value(json: serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::String(s),
        serde_json::Value::Array(arr) => Value::Array(arr.into_iter().map(json_to_value).collect()),
        serde_json::Value::Object(obj) => {
            let map: BTreeMap<String, Value> = obj
                .into_iter()
                .map(|(k, v)| (k, json_to_value(v)))
                .collect();
            Value::Map(map)
        }
    }
}

pub(crate) fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => serde_json::Value::Number((*i).into()),
        Value::Unsigned(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Bytes(b) => serde_json::Value::String(format!("bytes:{}", b.len())),
        Value::Array(arr) => serde_json::Value::Array(arr.iter().map(value_to_json).collect()),
        Value::Map(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
    }
}

/// A store that keeps every record exactly as written, raw or parsed, with
/// no tree semantics. Use `MemoryStore` unless a test needs `Record::Raw`.
#[derive(Default)]
pub(crate) struct RawMapStore {
    data: HashMap<Path, Record>,
}

impl RawMapStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.data.contains_key(path)
    }

    pub(crate) fn insert(&mut self, path: Path, record: Record) {
        self.data.insert(path, record);
    }
}

impl Reader for RawMapStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        Ok(self.data.get(from).cloned())
    }
}

impl Writer for RawMapStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.data.insert(to.clone(), data);
        Ok(to.clone())
    }
}
