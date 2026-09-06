//! JSON codec implementation.

use bytes::Bytes;
use structfs_core_store::{Codec, Error, Format, Value};

use crate::convert::{json_to_value, value_to_json};

/// A codec that handles JSON encoding/decoding.
///
/// This is the default codec for most use cases. It converts between
/// `Value` and JSON bytes.
///
/// # Example
///
/// ```rust
/// use structfs_serde_store::JsonCodec;
/// use structfs_core_store::{Codec, Format, Value};
/// use bytes::Bytes;
///
/// let codec = JsonCodec;
/// let value = Value::from("hello");
///
/// let bytes = codec.encode(&value, &Format::JSON).unwrap();
/// let decoded = codec.decode(&bytes, &Format::JSON).unwrap();
///
/// assert_eq!(decoded, value);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }

        let json: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| Error::decode(format.clone(), e.to_string()))?;

        Ok(json_to_value(json))
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }

        let json = value_to_json(value.clone());
        let bytes =
            serde_json::to_vec(&json).map_err(|e| Error::encode(format.clone(), e.to_string()))?;

        Ok(Bytes::from(bytes))
    }

    fn supports(&self, format: &Format) -> bool {
        format == &Format::JSON
    }
}

/// A codec for CBOR (`Format::CBOR`).
///
/// `Value` serializes structurally, so the mapping is natural — and
/// unlike JSON, `Value::Bytes` crosses as a CBOR byte string and comes
/// back as `Value::Bytes`, not an array of numbers.
#[derive(Debug, Clone, Copy, Default)]
pub struct CborCodec;

impl Codec for CborCodec {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        ciborium::from_reader(bytes.as_ref())
            .map_err(|e| Error::decode(format.clone(), e.to_string()))
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        let mut buffer = Vec::new();
        ciborium::into_writer(value, &mut buffer)
            .map_err(|e| Error::encode(format.clone(), e.to_string()))?;
        Ok(Bytes::from(buffer))
    }

    fn supports(&self, format: &Format) -> bool {
        format == &Format::CBOR
    }
}

/// A codec for FlexBuffers (`Format::FLEXBUFFERS`).
///
/// Like CBOR, the encoding is self-describing and byte-faithful:
/// `Value::Bytes` crosses as a blob and round-trips exactly.
#[derive(Debug, Clone, Copy, Default)]
pub struct FlexbuffersCodec;

impl Codec for FlexbuffersCodec {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        flexbuffers::from_slice(bytes).map_err(|e| Error::decode(format.clone(), e.to_string()))
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        flexbuffers::to_vec(value)
            .map(Bytes::from)
            .map_err(|e| Error::encode(format.clone(), e.to_string()))
    }

    fn supports(&self, format: &Format) -> bool {
        format == &Format::FLEXBUFFERS
    }
}

/// A codec that combines multiple codecs.
///
/// Routes encode/decode to the appropriate codec based on format.
pub struct MultiCodec {
    codecs: Vec<Box<dyn Codec>>,
}

impl MultiCodec {
    /// Create an empty multi-codec.
    pub fn new() -> Self {
        Self { codecs: Vec::new() }
    }

    /// Add a codec.
    pub fn add(&mut self, codec: impl Codec + 'static) {
        self.codecs.push(Box::new(codec));
    }

    /// Create a multi-codec with the JSON codec included.
    pub fn with_json() -> Self {
        let mut mc = Self::new();
        mc.add(JsonCodec);
        mc
    }

    /// The standard transports, all equivalent-tier: JSON, CBOR, and
    /// FlexBuffers, routed by format.
    pub fn standard() -> Self {
        let mut mc = Self::new();
        mc.add(JsonCodec);
        mc.add(CborCodec);
        mc.add(FlexbuffersCodec);
        mc
    }
}

impl Default for MultiCodec {
    fn default() -> Self {
        Self::standard()
    }
}

impl Codec for MultiCodec {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        for codec in &self.codecs {
            if codec.supports(format) {
                return codec.decode(bytes, format);
            }
        }
        Err(Error::UnsupportedFormat(format.clone()))
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        for codec in &self.codecs {
            if codec.supports(format) {
                return codec.encode(value, format);
            }
        }
        Err(Error::UnsupportedFormat(format.clone()))
    }

    fn supports(&self, format: &Format) -> bool {
        self.codecs.iter().any(|c| c.supports(format))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_codec_roundtrip() {
        let codec = JsonCodec;

        let original = Value::Map(
            [
                ("name".to_string(), Value::String("Alice".to_string())),
                ("age".to_string(), Value::Integer(30)),
            ]
            .into_iter()
            .collect(),
        );

        let bytes = codec.encode(&original, &Format::JSON).unwrap();
        let decoded = codec.decode(&bytes, &Format::JSON).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn json_codec_rejects_other_formats() {
        let codec = JsonCodec;

        let bytes = Bytes::from_static(b"hello");
        let result = codec.decode(&bytes, &Format::PROTOBUF);

        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn multi_codec_routes_correctly() {
        let codec = MultiCodec::with_json();

        assert!(codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::PROTOBUF));

        let value = Value::from("hello");
        let bytes = codec.encode(&value, &Format::JSON).unwrap();
        let decoded = codec.decode(&bytes, &Format::JSON).unwrap();

        assert_eq!(value, decoded);
    }

    #[test]
    fn json_codec_encode_unsupported_format() {
        let codec = JsonCodec;
        let value = Value::from("test");
        let result = codec.encode(&value, &Format::PROTOBUF);
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn json_codec_decode_invalid_json() {
        let codec = JsonCodec;
        let bytes = Bytes::from_static(b"not valid json {{{");
        let result = codec.decode(&bytes, &Format::JSON);
        assert!(matches!(
            result,
            Err(Error::Codec {
                operation: structfs_core_store::CodecOperation::Decode,
                ..
            })
        ));
    }

    #[test]
    fn multi_codec_decode_unsupported() {
        let codec = MultiCodec::new(); // Empty, no codecs
        let bytes = Bytes::from_static(b"hello");
        let result = codec.decode(&bytes, &Format::JSON);
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn multi_codec_encode_unsupported() {
        let codec = MultiCodec::new(); // Empty, no codecs
        let value = Value::from("test");
        let result = codec.encode(&value, &Format::JSON);
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn multi_codec_default() {
        let codec = MultiCodec::default();
        // Default includes JSON
        assert!(codec.supports(&Format::JSON));
    }

    #[test]
    fn multi_codec_add_custom() {
        use structfs_core_store::Codec as CoreCodec;

        struct CustomCodec;

        impl CoreCodec for CustomCodec {
            fn decode(&self, bytes: &Bytes, _format: &Format) -> Result<Value, Error> {
                Ok(Value::Bytes(bytes.to_vec()))
            }

            fn encode(&self, _value: &Value, _format: &Format) -> Result<Bytes, Error> {
                Ok(Bytes::from_static(b"custom"))
            }

            fn supports(&self, format: &Format) -> bool {
                format == &Format::OCTET_STREAM
            }
        }

        let mut codec = MultiCodec::new();
        codec.add(CustomCodec);

        assert!(codec.supports(&Format::OCTET_STREAM));
        assert!(!codec.supports(&Format::JSON));

        let decoded = codec
            .decode(&Bytes::from_static(b"data"), &Format::OCTET_STREAM)
            .unwrap();
        assert_eq!(decoded, Value::Bytes(b"data".to_vec()));

        let encoded = codec.encode(&Value::Null, &Format::OCTET_STREAM).unwrap();
        assert_eq!(encoded.as_ref(), b"custom");
    }

    #[test]
    fn json_codec_supports() {
        let codec = JsonCodec;
        assert!(codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::PROTOBUF));
        assert!(!codec.supports(&Format::OCTET_STREAM));
    }

    #[test]
    fn json_codec_default() {
        let codec: JsonCodec = Default::default();
        assert!(codec.supports(&Format::JSON));
    }

    #[test]
    fn json_codec_copy() {
        let codec1 = JsonCodec;
        let codec2 = codec1; // Copy
                             // Both should work the same
        let bytes = codec1.encode(&Value::from("test"), &Format::JSON).unwrap();
        let decoded = codec2.decode(&bytes, &Format::JSON).unwrap();
        assert_eq!(decoded, Value::from("test"));
    }

    #[test]
    fn json_codec_debug() {
        let codec = JsonCodec;
        let debug = format!("{:?}", codec);
        assert!(debug.contains("JsonCodec"));
    }

    fn sample() -> Value {
        Value::Map(
            [
                ("name".to_string(), Value::from("Alice")),
                ("age".to_string(), Value::Integer(30)),
                ("score".to_string(), Value::Float(0.5)),
                ("active".to_string(), Value::Bool(true)),
                ("note".to_string(), Value::Null),
                (
                    "tags".to_string(),
                    Value::Array(vec![Value::from("a"), Value::Integer(-7)]),
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    #[test]
    fn cbor_codec_roundtrip() {
        let codec = CborCodec;
        let bytes = codec.encode(&sample(), &Format::CBOR).unwrap();
        assert_eq!(codec.decode(&bytes, &Format::CBOR).unwrap(), sample());
    }

    #[test]
    fn flexbuffers_codec_roundtrip() {
        let codec = FlexbuffersCodec;
        let bytes = codec.encode(&sample(), &Format::FLEXBUFFERS).unwrap();
        assert_eq!(
            codec.decode(&bytes, &Format::FLEXBUFFERS).unwrap(),
            sample()
        );
    }

    #[test]
    fn bytes_survive_the_binary_transports() {
        // The property JSON lacks: Value::Bytes round-trips as bytes,
        // not as an array of numbers.
        let value = Value::Map(
            [("payload".to_string(), Value::Bytes(vec![0, 159, 146, 150]))]
                .into_iter()
                .collect(),
        );
        for (codec, format) in [
            (&CborCodec as &dyn Codec, Format::CBOR),
            (&FlexbuffersCodec as &dyn Codec, Format::FLEXBUFFERS),
        ] {
            let bytes = codec.encode(&value, &format).unwrap();
            assert_eq!(
                codec.decode(&bytes, &format).unwrap(),
                value,
                "bytes mangled by {format}"
            );
        }
    }

    #[test]
    fn binary_codecs_reject_other_formats() {
        assert!(matches!(
            CborCodec.encode(&Value::Null, &Format::JSON),
            Err(Error::UnsupportedFormat(_))
        ));
        assert!(matches!(
            FlexbuffersCodec.decode(&Bytes::from_static(b"x"), &Format::CBOR),
            Err(Error::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn binary_codecs_report_decode_errors() {
        let garbage = Bytes::from_static(&[0xff, 0xfe, 0xfd]);
        assert!(matches!(
            CborCodec.decode(&garbage, &Format::CBOR),
            Err(Error::Codec { .. })
        ));
        assert!(matches!(
            FlexbuffersCodec.decode(&Bytes::from_static(&[]), &Format::FLEXBUFFERS),
            Err(Error::Codec { .. })
        ));
    }

    #[test]
    fn standard_multi_codec_routes_every_transport() {
        let codec = MultiCodec::standard();
        for format in [Format::JSON, Format::CBOR, Format::FLEXBUFFERS] {
            assert!(codec.supports(&format), "missing transport: {format}");
            let bytes = codec.encode(&sample(), &format).unwrap();
            assert_eq!(codec.decode(&bytes, &format).unwrap(), sample());
        }
        assert!(!codec.supports(&Format::PROTOBUF));
    }

    #[test]
    fn multi_codec_supports_empty() {
        let codec = MultiCodec::new();
        assert!(!codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::PROTOBUF));
    }
}
