//! LazyRecord - a record with lazy parsing.
//!
//! This type is useful when you might or might not need to parse data.
//! It caches the parsed result, so repeated access is cheap.

use std::sync::{Mutex, OnceLock};

use bytes::Bytes;

use crate::{Codec, Error, Format, Value};

/// A record with lazy parsing. Thread-safe.
///
/// Unlike `Record`, which is either raw or parsed, `LazyRecord` can be
/// both simultaneously. It caches the parsed result on first access.
///
/// No workspace crate uses it yet: it is kept as a documented public
/// contract (the 0.4 migration pins its retryable-error, serialized-init
/// behaviour, tested in `tests/coherent_contracts.rs`) for middleware
/// outside this repository.
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
/// `LazyRecord` is `Send + Sync` (every field is; nothing is asserted by
/// hand). Multiple threads can safely call `value()` concurrently - only one
/// will actually parse, others will wait and get the cached result.
pub struct LazyRecord {
    /// The raw bytes and format (if created from raw data).
    raw: Option<(Bytes, Format)>,
    /// The parsed value, populated on first access to `value()`.
    parsed: OnceLock<Value>,
    decoding: Mutex<()>,
}

impl LazyRecord {
    /// Create a lazy record from raw bytes.
    ///
    /// The bytes will be parsed on first call to `value()`.
    pub fn from_raw(bytes: Bytes, format: Format) -> Self {
        Self {
            raw: Some((bytes, format)),
            parsed: OnceLock::new(),
            decoding: Mutex::new(()),
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
            decoding: Mutex::new(()),
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
            decoding: Mutex::new(()),
        }
    }

    /// Get the value, parsing if necessary.
    ///
    /// This method is thread-safe. If multiple threads call it concurrently,
    /// only one will actually parse; others will wait and get the cached result.
    ///
    /// Decode errors are not cached; later calls retry. The first successful
    /// codec determines the cached value. A codec must not recursively decode
    /// this same record.
    ///
    /// # Errors
    ///
    /// Returns an error if the codec fails to parse the bytes, or (with a
    /// record that somehow holds neither bytes nor a value) an `Error::Store`
    /// from the `lazy_record` store. Every public constructor supplies one of
    /// the two, so the latter is unreachable through this API. This method
    /// does not panic.
    pub fn value(&self, codec: &dyn Codec) -> Result<&Value, Error> {
        // Fast path: already parsed
        if let Some(v) = self.parsed.get() {
            return Ok(v);
        }

        // Serialize fallible initialization; errors are not cached. A later call
        // can retry with another codec. A panicked decoder publishes no value.
        let _guard = self.decoding.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = self.parsed.get() {
            return Ok(value);
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
    /// A record holding neither (unreachable through the public
    /// constructors) is an `Error::Store` rather than a fabricated
    /// `Parsed(Null)`.
    pub fn into_record(self) -> Result<crate::Record, Error> {
        if let Some(value) = self.parsed.into_inner() {
            Ok(crate::Record::Parsed(value))
        } else if let Some((bytes, format)) = self.raw {
            Ok(crate::Record::Raw { bytes, format })
        } else {
            Err(Error::store(
                "lazy_record",
                "into_record",
                "LazyRecord holds neither bytes nor a value",
            ))
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
            decoding: Mutex::new(()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestJsonCodec;

    #[test]
    fn lazy_record_is_send_and_sync_without_unsafe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LazyRecord>();
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
        let converted = record.into_record().unwrap();
        assert!(matches!(converted, crate::Record::Raw { .. }));

        // Parsed -> Parsed
        let record = LazyRecord::from_parsed(Value::from("test"));
        let converted = record.into_record().unwrap();
        assert!(matches!(converted, crate::Record::Parsed(_)));

        // Neither (constructible only inside the crate) -> error, not Null
        let empty = LazyRecord {
            raw: None,
            parsed: OnceLock::new(),
            decoding: Mutex::new(()),
        };
        assert!(matches!(empty.into_record(), Err(Error::Store { .. })));
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
        let converted = record.into_record().unwrap();
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
    fn decode_error_is_not_cached() {
        let record = LazyRecord::from_raw(Bytes::from_static(b"not valid json{"), Format::JSON);
        let codec = TestJsonCodec;
        assert!(record.value(&codec).is_err());
        assert!(!record.is_parsed());
        // A record without bytes and without a value reports a store error.
        let empty = LazyRecord {
            raw: None,
            parsed: OnceLock::new(),
            decoding: Mutex::new(()),
        };
        assert!(matches!(empty.value(&codec), Err(Error::Store { .. })));
    }
}
