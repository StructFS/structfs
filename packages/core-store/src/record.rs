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
}
