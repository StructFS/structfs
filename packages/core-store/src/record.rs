//! The Record type - maybe-parsed data with format hint.

use bytes::Bytes;

use crate::{Codec, Error, Format, Value};

/// A record that can be forwarded without parsing or parsed for inspection.
///
/// This is the core abstraction for zero-copy forwarding. A Record is either:
/// - `Raw`: Unparsed bytes with a format hint. Can be forwarded without parsing.
/// - `Parsed`: A parsed `Value` tree. Efficient for inspection and modification.
///
/// # Zero-Copy Forwarding
///
/// ```rust
/// use structfs_core_store::{Record, Format};
/// use bytes::Bytes;
///
/// // Data comes in as raw bytes
/// let record = Record::raw(Bytes::from_static(b"{\"name\":\"Alice\"}"), Format::JSON);
///
/// // Forward without parsing - just pass the Record through
/// // No JSON parsing happens!
/// ```
///
/// # Lazy Parsing
///
/// ```rust
/// use structfs_core_store::{Record, Format, Value};
/// use bytes::Bytes;
///
/// let record = Record::raw(Bytes::from_static(b"..."), Format::JSON);
///
/// // Only parse when you need to inspect
/// // let value = record.into_value(&codec)?;
/// ```
#[derive(Clone)]
pub enum Record {
    /// Unparsed bytes with format hint.
    ///
    /// The bytes can be forwarded without parsing. Use `into_value()` to
    /// parse when you need to inspect or modify the data.
    Raw {
        /// The raw bytes.
        bytes: Bytes,
        /// Hint about the format (JSON, protobuf, etc.)
        format: Format,
    },

    /// Parsed tree structure.
    ///
    /// Efficient for inspection and modification. Use `into_bytes()` to
    /// serialize when you need to send over the wire.
    Parsed(Value),
}

impl Record {
    // === Construction ===

    /// Create a record from raw bytes.
    pub fn raw(bytes: impl Into<Bytes>, format: Format) -> Self {
        Record::Raw {
            bytes: bytes.into(),
            format,
        }
    }

    /// Create a record from a parsed value.
    pub fn parsed(value: Value) -> Self {
        Record::Parsed(value)
    }

    // === Inspection (cheap) ===

    /// Check if this record is in raw (unparsed) form.
    pub fn is_raw(&self) -> bool {
        matches!(self, Record::Raw { .. })
    }

    /// Check if this record is parsed.
    pub fn is_parsed(&self) -> bool {
        matches!(self, Record::Parsed(_))
    }

    /// Get the format hint.
    ///
    /// For `Parsed` records, returns `Format::VALUE`.
    pub fn format(&self) -> Format {
        match self {
            Record::Raw { format, .. } => format.clone(),
            Record::Parsed(_) => Format::VALUE,
        }
    }

    /// Get raw bytes if available without serialization.
    ///
    /// Returns `None` for `Parsed` records (would require serialization).
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Record::Raw { bytes, .. } => Some(bytes),
            Record::Parsed(_) => None,
        }
    }

    /// Get parsed value if available without parsing.
    ///
    /// Returns `None` for `Raw` records (would require parsing).
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            Record::Raw { .. } => None,
            Record::Parsed(v) => Some(v),
        }
    }

    // === Conversion (potentially costly) ===

    /// Parse into a Value.
    ///
    /// - For `Parsed` records: returns the value (no cost).
    /// - For `Raw` records: parses the bytes using the codec.
    ///
    /// This is where you pay the parsing cost.
    pub fn into_value(self, codec: &dyn Codec) -> Result<Value, Error> {
        match self {
            Record::Parsed(v) => Ok(v),
            Record::Raw { bytes, format } => codec.decode(&bytes, &format),
        }
    }

    /// Serialize into bytes.
    ///
    /// - For `Raw` records with matching format: returns the bytes (no cost).
    /// - For `Raw` records with different format: transcodes via Value.
    /// - For `Parsed` records: serializes using the codec.
    ///
    /// This is where you pay the serialization cost.
    pub fn into_bytes(self, codec: &dyn Codec, target_format: &Format) -> Result<Bytes, Error> {
        match self {
            Record::Raw { bytes, format } if &format == target_format => Ok(bytes),
            Record::Raw { bytes, format } => {
                // Transcode: parse then re-serialize
                let value = codec.decode(&bytes, &format)?;
                codec.encode(&value, target_format)
            }
            Record::Parsed(v) => codec.encode(&v, target_format),
        }
    }

    /// Try to get bytes without serialization, returning the record if not possible.
    ///
    /// Useful when you want bytes if available, but don't want to pay serialization cost.
    pub fn try_into_bytes(self, target_format: &Format) -> Result<Bytes, Self> {
        match self {
            Record::Raw { bytes, format } if &format == target_format => Ok(bytes),
            other => Err(other),
        }
    }
}

impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Record::Raw { bytes, format } => f
                .debug_struct("Record::Raw")
                .field("bytes_len", &bytes.len())
                .field("format", format)
                .finish(),
            Record::Parsed(v) => f.debug_tuple("Record::Parsed").field(v).finish(),
        }
    }
}

impl From<Value> for Record {
    fn from(v: Value) -> Self {
        Record::Parsed(v)
    }
}

impl From<Bytes> for Record {
    fn from(bytes: Bytes) -> Self {
        Record::Raw {
            bytes,
            format: Format::OCTET_STREAM,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Simple test codec that handles JSON
    struct TestJsonCodec;

    impl Codec for TestJsonCodec {
        fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
            if format != &Format::JSON {
                return Err(Error::UnsupportedFormat(format.clone()));
            }
            let json: serde_json::Value =
                serde_json::from_slice(bytes).map_err(|e| Error::Decode {
                    format: format.clone(),
                    message: e.to_string(),
                })?;
            Ok(json_to_value(json))
        }

        fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
            if format != &Format::JSON {
                return Err(Error::UnsupportedFormat(format.clone()));
            }
            let json = value_to_json(value);
            let bytes = serde_json::to_vec(&json).map_err(|e| Error::Encode {
                format: format.clone(),
                message: e.to_string(),
            })?;
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

    #[test]
    fn raw_record_inspection() {
        let record = Record::raw(Bytes::from_static(b"hello"), Format::JSON);

        assert!(record.is_raw());
        assert!(!record.is_parsed());
        assert_eq!(record.format(), Format::JSON);
        assert_eq!(record.as_bytes(), Some(&Bytes::from_static(b"hello")));
        assert_eq!(record.as_value(), None);
    }

    #[test]
    fn parsed_record_inspection() {
        let record = Record::parsed(Value::from("hello"));

        assert!(!record.is_raw());
        assert!(record.is_parsed());
        assert_eq!(record.format(), Format::VALUE);
        assert_eq!(record.as_bytes(), None);
        assert_eq!(record.as_value(), Some(&Value::from("hello")));
    }

    #[test]
    fn try_into_bytes_matching_format() {
        let bytes = Bytes::from_static(b"hello");
        let record = Record::raw(bytes.clone(), Format::JSON);

        let result = record.try_into_bytes(&Format::JSON);
        assert_eq!(result.unwrap(), bytes);
    }

    #[test]
    fn try_into_bytes_mismatched_format() {
        let record = Record::raw(Bytes::from_static(b"hello"), Format::JSON);

        let result = record.try_into_bytes(&Format::PROTOBUF);
        assert!(result.is_err()); // Returns the record back
    }

    #[test]
    fn into_value_parsed() {
        let codec = TestJsonCodec;
        let record = Record::parsed(Value::from("hello"));
        let value = record.into_value(&codec).unwrap();
        assert_eq!(value, Value::String("hello".to_string()));
    }

    #[test]
    fn into_value_raw() {
        let codec = TestJsonCodec;
        let record = Record::raw(Bytes::from_static(b"{\"name\":\"Alice\"}"), Format::JSON);
        let value = record.into_value(&codec).unwrap();
        match value {
            Value::Map(map) => {
                assert_eq!(map.get("name"), Some(&Value::String("Alice".to_string())));
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn into_bytes_raw_matching_format() {
        let codec = TestJsonCodec;
        let bytes = Bytes::from_static(b"{\"a\":1}");
        let record = Record::raw(bytes.clone(), Format::JSON);
        let result = record.into_bytes(&codec, &Format::JSON).unwrap();
        assert_eq!(result, bytes);
    }

    #[test]
    fn into_bytes_parsed() {
        let codec = TestJsonCodec;
        let record = Record::parsed(Value::from("hello"));
        let result = record.into_bytes(&codec, &Format::JSON).unwrap();
        assert_eq!(result, Bytes::from_static(b"\"hello\""));
    }

    #[test]
    fn into_bytes_raw_different_format_error() {
        let codec = TestJsonCodec;
        let record = Record::raw(Bytes::from_static(b"data"), Format::JSON);
        // Trying to transcode to PROTOBUF, which our codec doesn't support
        let result = record.into_bytes(&codec, &Format::PROTOBUF);
        assert!(result.is_err());
    }

    #[test]
    fn try_into_bytes_parsed_returns_err() {
        let record = Record::parsed(Value::Null);
        let result = record.try_into_bytes(&Format::JSON);
        assert!(result.is_err());
    }

    #[test]
    fn debug_raw_record() {
        let record = Record::raw(Bytes::from_static(b"hello"), Format::JSON);
        let debug = format!("{:?}", record);
        assert!(debug.contains("Record::Raw"));
        assert!(debug.contains("bytes_len"));
        assert!(debug.contains("format"));
    }

    #[test]
    fn debug_parsed_record() {
        let record = Record::parsed(Value::from(42));
        let debug = format!("{:?}", record);
        assert!(debug.contains("Record::Parsed"));
    }

    #[test]
    fn from_value_impl() {
        let value = Value::from("test");
        let record: Record = value.into();
        assert!(record.is_parsed());
        assert_eq!(record.as_value(), Some(&Value::String("test".to_string())));
    }

    #[test]
    fn from_bytes_impl() {
        let bytes = Bytes::from_static(b"test data");
        let record: Record = bytes.clone().into();
        assert!(record.is_raw());
        assert_eq!(record.format(), Format::OCTET_STREAM);
        assert_eq!(record.as_bytes(), Some(&bytes));
    }

    #[test]
    fn clone_raw_record() {
        let record = Record::raw(Bytes::from_static(b"data"), Format::JSON);
        let cloned = record.clone();
        assert!(cloned.is_raw());
        assert_eq!(cloned.format(), Format::JSON);
    }

    #[test]
    fn clone_parsed_record() {
        let record = Record::parsed(Value::from(123));
        let cloned = record.clone();
        assert!(cloned.is_parsed());
        assert_eq!(cloned.as_value(), Some(&Value::Integer(123)));
    }

    #[test]
    fn raw_with_vec_bytes() {
        let vec = vec![1u8, 2, 3, 4];
        let record = Record::raw(vec, Format::OCTET_STREAM);
        assert!(record.is_raw());
        assert_eq!(record.as_bytes().map(|b| b.len()), Some(4));
    }

    /// Codec that supports both JSON and a custom format
    struct MultiFormatCodec;

    impl Codec for MultiFormatCodec {
        fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
            if format == &Format::JSON {
                let json: serde_json::Value =
                    serde_json::from_slice(bytes).map_err(|e| Error::Decode {
                        format: format.clone(),
                        message: e.to_string(),
                    })?;
                Ok(json_to_value(json))
            } else if format.as_str() == "text/plain" {
                let s = String::from_utf8_lossy(bytes);
                Ok(Value::String(s.to_string()))
            } else {
                Err(Error::UnsupportedFormat(format.clone()))
            }
        }

        fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
            if format == &Format::JSON {
                let json = value_to_json(value);
                let bytes = serde_json::to_vec(&json).map_err(|e| Error::Encode {
                    format: format.clone(),
                    message: e.to_string(),
                })?;
                Ok(Bytes::from(bytes))
            } else if format.as_str() == "text/plain" {
                match value {
                    Value::String(s) => Ok(Bytes::from(s.clone())),
                    _ => Ok(Bytes::from(format!("{:?}", value))),
                }
            } else {
                Err(Error::UnsupportedFormat(format.clone()))
            }
        }

        fn supports(&self, format: &Format) -> bool {
            format == &Format::JSON || format.as_str() == "text/plain"
        }
    }

    #[test]
    fn into_bytes_transcode() {
        // Test transcoding: Raw JSON -> text/plain
        let codec = MultiFormatCodec;
        let text_format = Format::new("text/plain");

        // Create a raw JSON record
        let record = Record::raw(Bytes::from_static(b"\"hello world\""), Format::JSON);

        // Transcode to text/plain
        let result = record.into_bytes(&codec, &text_format).unwrap();

        // The JSON string "hello world" should be decoded then re-encoded as text/plain
        assert_eq!(result.as_ref(), b"hello world");
    }

    #[test]
    fn into_bytes_transcode_decode_error() {
        // Test transcode error when decode fails
        let codec = MultiFormatCodec;
        let text_format = Format::new("text/plain");

        // Create a raw record with invalid JSON
        let record = Record::raw(Bytes::from_static(b"not valid json {{{"), Format::JSON);

        // Transcode should fail during decode phase
        let result = record.into_bytes(&codec, &text_format);
        assert!(result.is_err());
    }

    #[test]
    fn into_value_decode_error() {
        let codec = TestJsonCodec;
        let record = Record::raw(Bytes::from_static(b"not valid json"), Format::JSON);
        let result = record.into_value(&codec);
        assert!(result.is_err());
    }

    #[test]
    fn into_bytes_parsed_encode_error() {
        let codec = TestJsonCodec;
        let record = Record::parsed(Value::from("test"));
        // Try to encode as PROTOBUF which TestJsonCodec doesn't support
        let result = record.into_bytes(&codec, &Format::PROTOBUF);
        assert!(result.is_err());
    }

    #[test]
    fn format_raw_format_clone() {
        // Verify that format() returns a cloned format, not a reference
        let record = Record::raw(Bytes::from_static(b"data"), Format::new("custom/format"));
        let format = record.format();
        assert_eq!(format.as_str(), "custom/format");
    }

    #[test]
    fn into_value_unsupported_format() {
        let codec = TestJsonCodec;
        let record = Record::raw(Bytes::from_static(b"data"), Format::PROTOBUF);
        let result = record.into_value(&codec);
        assert!(result.is_err());
    }
}
