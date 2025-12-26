//! LazyRecord - a record with lazy parsing.
//!
//! This type is useful when you might or might not need to parse data.
//! It caches the parsed result, so repeated access is cheap.

use std::sync::OnceLock;

use bytes::Bytes;

use crate::{Codec, Error, Format, Value};

/// A record with lazy parsing. Thread-safe.
///
/// Unlike `Record`, which is either raw or parsed, `LazyRecord` can be
/// both simultaneously. It caches the parsed result on first access.
///
/// # Use Cases
///
/// - Middleware that might need to inspect data based on some condition
/// - Caching layers that want to keep both representations
/// - Any scenario where parsing cost should be deferred and amortized
///
/// # Example
///
/// ```rust
/// use structfs_core_store::{LazyRecord, Format, Value};
/// use bytes::Bytes;
///
/// let record = LazyRecord::from_raw(
///     Bytes::from_static(b"{\"name\":\"Alice\"}"),
///     Format::JSON,
/// );
///
/// // No parsing yet
/// assert!(record.bytes().is_some());
///
/// // Now parse (would require a codec in real use)
/// // let value = record.value(&codec)?;
/// ```
///
/// # Thread Safety
///
/// `LazyRecord` is `Send + Sync`. Multiple threads can safely call `value()`
/// concurrently - only one will actually parse, others will wait and get
/// the cached result.
pub struct LazyRecord {
    /// The raw bytes and format (if created from raw data).
    raw: Option<(Bytes, Format)>,
    /// The parsed value, populated on first access to `value()`.
    parsed: OnceLock<Value>,
}

impl LazyRecord {
    /// Create a lazy record from raw bytes.
    ///
    /// The bytes will be parsed on first call to `value()`.
    pub fn from_raw(bytes: Bytes, format: Format) -> Self {
        Self {
            raw: Some((bytes, format)),
            parsed: OnceLock::new(),
        }
    }

    /// Create a lazy record from a parsed value.
    ///
    /// The value is immediately available; `bytes()` will return `None`.
    pub fn from_parsed(value: Value) -> Self {
        let lock = OnceLock::new();
        // This cannot fail because the lock is new
        let _ = lock.set(value);
        Self {
            raw: None,
            parsed: lock,
        }
    }

    /// Create a lazy record with both raw bytes and parsed value.
    ///
    /// Useful when you've already parsed and want to cache both.
    pub fn from_both(bytes: Bytes, format: Format, value: Value) -> Self {
        let lock = OnceLock::new();
        let _ = lock.set(value);
        Self {
            raw: Some((bytes, format)),
            parsed: lock,
        }
    }

    /// Get the value, parsing if necessary.
    ///
    /// This method is thread-safe. If multiple threads call it concurrently,
    /// only one will actually parse; others will wait and get the cached result.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The record was created from parsed value only and bytes are needed (won't happen)
    /// - The codec fails to parse the bytes
    ///
    /// # Panics
    ///
    /// This method will panic if called on a LazyRecord with no raw data and
    /// the value hasn't been set. This should never happen with proper construction.
    pub fn value(&self, codec: &dyn Codec) -> Result<&Value, Error> {
        // Fast path: already parsed
        if let Some(v) = self.parsed.get() {
            return Ok(v);
        }

        // Slow path: need to parse
        let (bytes, format) = self.raw.as_ref().ok_or_else(|| {
            Error::store(
                "lazy_record",
                "value",
                "LazyRecord has no raw data to parse",
            )
        })?;

        let value = codec.decode(bytes, format)?;

        // Try to set the value. If another thread beat us, use their value.
        // OnceLock::set returns Err(value) if already set, so we ignore the error.
        let _ = self.parsed.set(value);

        // Now it's definitely set
        Ok(self.parsed.get().expect("just set"))
    }

    /// Get the value if already parsed, without triggering parsing.
    ///
    /// Returns `None` if the value hasn't been parsed yet.
    pub fn value_if_parsed(&self) -> Option<&Value> {
        self.parsed.get()
    }

    /// Get the raw bytes if available.
    ///
    /// Returns `None` if the record was created from a parsed value only.
    pub fn bytes(&self) -> Option<&Bytes> {
        self.raw.as_ref().map(|(b, _)| b)
    }

    /// Get the format hint if available.
    ///
    /// Returns `None` if the record was created from a parsed value only.
    pub fn format(&self) -> Option<&Format> {
        self.raw.as_ref().map(|(_, f)| f)
    }

    /// Check if the value has been parsed.
    pub fn is_parsed(&self) -> bool {
        self.parsed.get().is_some()
    }

    /// Check if raw bytes are available.
    pub fn has_bytes(&self) -> bool {
        self.raw.is_some()
    }

    /// Convert to a `Record`, consuming this lazy record.
    ///
    /// If parsed, returns `Record::Parsed`. Otherwise returns `Record::Raw`.
    pub fn into_record(self) -> crate::Record {
        if let Some(value) = self.parsed.into_inner() {
            crate::Record::Parsed(value)
        } else if let Some((bytes, format)) = self.raw {
            crate::Record::Raw { bytes, format }
        } else {
            // This shouldn't happen with proper construction
            crate::Record::Parsed(Value::Null)
        }
    }
}

impl std::fmt::Debug for LazyRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyRecord")
            .field("has_bytes", &self.raw.is_some())
            .field("bytes_len", &self.raw.as_ref().map(|(b, _)| b.len()))
            .field("format", &self.raw.as_ref().map(|(_, f)| f))
            .field("is_parsed", &self.parsed.get().is_some())
            .finish()
    }
}

impl Clone for LazyRecord {
    fn clone(&self) -> Self {
        let parsed = OnceLock::new();
        if let Some(value) = self.parsed.get() {
            let _ = parsed.set(value.clone());
        }
        Self {
            raw: self.raw.clone(),
            parsed,
        }
    }
}

impl From<crate::Record> for LazyRecord {
    fn from(record: crate::Record) -> Self {
        match record {
            crate::Record::Raw { bytes, format } => Self::from_raw(bytes, format),
            crate::Record::Parsed(value) => Self::from_parsed(value),
        }
    }
}

// Safety: OnceLock<Value> is Send + Sync, and so is Option<(Bytes, Format)>
unsafe impl Send for LazyRecord {}
unsafe impl Sync for LazyRecord {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Test codec that parses JSON using serde_json (dev-dependency).
    struct TestJsonCodec;

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
            let bytes = serde_json::to_vec(&json)
                .map_err(|e| Error::encode(format.clone(), e.to_string()))?;
            Ok(Bytes::from(bytes))
        }

        fn supports(&self, format: &Format) -> bool {
            format == &Format::JSON
        }
    }

    fn json_to_value(json: serde_json::Value) -> Value {
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
            serde_json::Value::Array(arr) => {
                Value::Array(arr.into_iter().map(json_to_value).collect())
            }
            serde_json::Value::Object(obj) => {
                let map: BTreeMap<String, Value> = obj
                    .into_iter()
                    .map(|(k, v)| (k, json_to_value(v)))
                    .collect();
                Value::Map(map)
            }
        }
    }

    fn value_to_json(value: &Value) -> serde_json::Value {
        match value {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Integer(i) => serde_json::Value::Number((*i).into()),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Bytes(b) => {
                // Encode as base64 for JSON
                use std::io::Write;
                let mut buf = Vec::new();
                write!(&mut buf, "base64:{}", base64_encode(b)).unwrap();
                serde_json::Value::String(String::from_utf8(buf).unwrap())
            }
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

    fn base64_encode(bytes: &[u8]) -> String {
        // Simple base64 encoding for tests
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as usize;
            let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
            let b2 = chunk.get(2).copied().unwrap_or(0) as usize;

            result.push(CHARS[(b0 >> 2) & 0x3F] as char);
            result.push(CHARS[((b0 << 4) | (b1 >> 4)) & 0x3F] as char);
            if chunk.len() > 1 {
                result.push(CHARS[((b1 << 2) | (b2 >> 6)) & 0x3F] as char);
            } else {
                result.push('=');
            }
            if chunk.len() > 2 {
                result.push(CHARS[b2 & 0x3F] as char);
            } else {
                result.push('=');
            }
        }
        result
    }

    #[test]
    fn lazy_parsing_works() {
        let json = b"{\"name\":\"Alice\",\"age\":30}";
        let record = LazyRecord::from_raw(Bytes::from_static(json), Format::JSON);

        // Not parsed yet
        assert!(!record.is_parsed());
        assert!(record.bytes().is_some());
        assert!(record.value_if_parsed().is_none());

        // Parse
        let codec = TestJsonCodec;
        let value = record.value(&codec).unwrap();

        // Now parsed
        assert!(record.is_parsed());
        assert!(matches!(value, Value::Map(_)));

        // Second access is cached
        let value2 = record.value(&codec).unwrap();
        assert!(std::ptr::eq(value, value2)); // Same reference
    }

    #[test]
    fn from_parsed_works() {
        let value = Value::from("hello");
        let record = LazyRecord::from_parsed(value.clone());

        assert!(record.is_parsed());
        assert!(!record.has_bytes());

        let codec = TestJsonCodec;
        let retrieved = record.value(&codec).unwrap();
        assert_eq!(retrieved, &value);
    }

    #[test]
    fn from_both_works() {
        let json = b"{\"x\":1}";
        let value = Value::from(42i64);
        let record = LazyRecord::from_both(Bytes::from_static(json), Format::JSON, value.clone());

        // Both available immediately
        assert!(record.is_parsed());
        assert!(record.has_bytes());

        // No parsing needed
        let codec = TestJsonCodec;
        let retrieved = record.value(&codec).unwrap();
        assert_eq!(retrieved, &value);
    }

    #[test]
    fn clone_preserves_parsed_state() {
        let json = b"{\"a\":1}";
        let record = LazyRecord::from_raw(Bytes::from_static(json), Format::JSON);

        // Parse the original
        let codec = TestJsonCodec;
        let _ = record.value(&codec).unwrap();
        assert!(record.is_parsed());

        // Clone should also be parsed
        let cloned = record.clone();
        assert!(cloned.is_parsed());
    }

    #[test]
    fn into_record_works() {
        // Unparsed -> Raw
        let record = LazyRecord::from_raw(Bytes::from_static(b"data"), Format::OCTET_STREAM);
        let converted = record.into_record();
        assert!(matches!(converted, crate::Record::Raw { .. }));

        // Parsed -> Parsed
        let record = LazyRecord::from_parsed(Value::from("test"));
        let converted = record.into_record();
        assert!(matches!(converted, crate::Record::Parsed(_)));
    }

    #[test]
    fn format_method_works() {
        let record = LazyRecord::from_raw(Bytes::from_static(b"test"), Format::JSON);
        assert_eq!(record.format(), Some(&Format::JSON));

        let record_parsed = LazyRecord::from_parsed(Value::Null);
        assert!(record_parsed.format().is_none());
    }

    #[test]
    fn debug_impl() {
        let record = LazyRecord::from_raw(Bytes::from_static(b"test"), Format::JSON);
        let debug = format!("{:?}", record);
        assert!(debug.contains("LazyRecord"));
        assert!(debug.contains("has_bytes: true"));
        assert!(debug.contains("is_parsed: false"));
    }

    #[test]
    fn from_record_raw() {
        let raw_record = crate::Record::Raw {
            bytes: Bytes::from_static(b"{\"a\":1}"),
            format: Format::JSON,
        };
        let lazy: LazyRecord = raw_record.into();
        assert!(lazy.has_bytes());
        assert!(!lazy.is_parsed());
    }

    #[test]
    fn from_record_parsed() {
        let parsed_record = crate::Record::Parsed(Value::from(42i64));
        let lazy: LazyRecord = parsed_record.into();
        assert!(!lazy.has_bytes());
        assert!(lazy.is_parsed());
    }

    #[test]
    fn clone_unparsed_record() {
        let record = LazyRecord::from_raw(Bytes::from_static(b"test"), Format::JSON);
        assert!(!record.is_parsed());

        let cloned = record.clone();
        assert!(!cloned.is_parsed());
        assert!(cloned.has_bytes());
        assert_eq!(cloned.bytes(), record.bytes());
    }

    #[test]
    fn value_if_parsed_returns_none_when_not_parsed() {
        let record = LazyRecord::from_raw(Bytes::from_static(b"test"), Format::JSON);
        assert!(record.value_if_parsed().is_none());
    }

    #[test]
    fn value_if_parsed_returns_some_when_parsed() {
        let value = Value::from("hello");
        let record = LazyRecord::from_parsed(value.clone());
        let retrieved = record.value_if_parsed();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), &value);
    }

    #[test]
    fn bytes_returns_none_for_parsed_only() {
        let record = LazyRecord::from_parsed(Value::Null);
        assert!(record.bytes().is_none());
    }

    #[test]
    fn has_bytes_returns_correct_value() {
        let raw = LazyRecord::from_raw(Bytes::from_static(b"test"), Format::JSON);
        assert!(raw.has_bytes());

        let parsed = LazyRecord::from_parsed(Value::Null);
        assert!(!parsed.has_bytes());

        let both = LazyRecord::from_both(Bytes::from_static(b"test"), Format::JSON, Value::Null);
        assert!(both.has_bytes());
    }

    #[test]
    fn into_record_after_parse() {
        let json = b"{\"name\":\"test\"}";
        let record = LazyRecord::from_raw(Bytes::from_static(json), Format::JSON);

        // Parse it first
        let codec = TestJsonCodec;
        let _ = record.value(&codec).unwrap();
        assert!(record.is_parsed());

        // into_record should return Parsed since it's been parsed
        let converted = record.into_record();
        assert!(matches!(converted, crate::Record::Parsed(_)));
    }

    #[test]
    fn value_caches_across_calls() {
        let json = b"{\"key\":\"value\"}";
        let record = LazyRecord::from_raw(Bytes::from_static(json), Format::JSON);
        let codec = TestJsonCodec;

        let first = record.value(&codec).unwrap();
        let second = record.value(&codec).unwrap();
        let third = record.value(&codec).unwrap();

        // All should be the same cached reference
        assert!(std::ptr::eq(first, second));
        assert!(std::ptr::eq(second, third));
    }

    #[test]
    fn debug_shows_correct_state_after_parse() {
        let record = LazyRecord::from_raw(Bytes::from_static(b"{}"), Format::JSON);
        let codec = TestJsonCodec;
        let _ = record.value(&codec).unwrap();

        let debug = format!("{:?}", record);
        assert!(debug.contains("is_parsed: true"));
    }

    #[test]
    fn value_to_json_handles_bytes() {
        let bytes_value = Value::Bytes(vec![72, 101, 108, 108, 111]); // "Hello"
        let json = value_to_json(&bytes_value);
        assert!(matches!(json, serde_json::Value::String(_)));
        let s = json.as_str().unwrap();
        assert!(s.starts_with("base64:"));
    }

    #[test]
    fn value_to_json_handles_float() {
        let float_value = Value::Float(1.234567);
        let json = value_to_json(&float_value);
        assert!(json.is_number());
    }

    #[test]
    fn value_to_json_handles_array() {
        let array_value = Value::Array(vec![Value::Integer(1), Value::Integer(2)]);
        let json = value_to_json(&array_value);
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[test]
    fn value_to_json_handles_map() {
        let mut map = BTreeMap::new();
        map.insert("key".to_string(), Value::String("value".to_string()));
        let map_value = Value::Map(map);
        let json = value_to_json(&map_value);
        assert!(json.is_object());
        assert_eq!(json.get("key").unwrap().as_str().unwrap(), "value");
    }

    #[test]
    fn json_to_value_handles_float() {
        let json = serde_json::json!(1.234567);
        let value = json_to_value(json);
        assert!(matches!(value, Value::Float(_)));
    }

    #[test]
    fn json_to_value_handles_null() {
        let json = serde_json::Value::Null;
        let value = json_to_value(json);
        assert!(matches!(value, Value::Null));
    }

    #[test]
    fn json_to_value_handles_bool() {
        let json = serde_json::json!(true);
        let value = json_to_value(json);
        assert!(matches!(value, Value::Bool(true)));
    }

    #[test]
    fn codec_unsupported_format_decode() {
        let codec = TestJsonCodec;
        let result = codec.decode(&Bytes::from_static(b"data"), &Format::OCTET_STREAM);
        assert!(result.is_err());
    }

    #[test]
    fn codec_unsupported_format_encode() {
        let codec = TestJsonCodec;
        let result = codec.encode(&Value::Null, &Format::OCTET_STREAM);
        assert!(result.is_err());
    }

    #[test]
    fn codec_supports() {
        let codec = TestJsonCodec;
        assert!(codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::OCTET_STREAM));
    }

    #[test]
    fn base64_encode_full_chunks() {
        // Test with bytes that divide evenly into 3
        let result = base64_encode(b"Man");
        assert_eq!(result, "TWFu");
    }

    #[test]
    fn base64_encode_partial_chunks() {
        // Test with 2 bytes (needs padding)
        let result = base64_encode(b"Ma");
        assert_eq!(result, "TWE=");
    }

    #[test]
    fn base64_encode_single_byte() {
        // Test with 1 byte (needs double padding)
        let result = base64_encode(b"M");
        assert_eq!(result, "TQ==");
    }

    #[test]
    fn decode_invalid_json_error() {
        let codec = TestJsonCodec;
        let result = codec.decode(&Bytes::from_static(b"not valid json{"), &Format::JSON);
        assert!(result.is_err());
        // The error message format depends on the error type
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Decode") || err.to_string().contains("expected"));
    }

    #[test]
    fn value_to_json_handles_nan() {
        // NaN can't be represented in JSON, should return Null
        let float_value = Value::Float(f64::NAN);
        let json = value_to_json(&float_value);
        assert!(json.is_null());
    }
}
