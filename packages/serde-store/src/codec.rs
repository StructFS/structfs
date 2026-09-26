//! Versioned, bounded value codecs.
use crate::{limits::Failure, Limits};
use bytes::Bytes;
use structfs_core_store::{Codec, CodecErrorKind, CodecOperation, Error, Format, Value};

/// Explicitly selected StructFS v1 codec contract.
///
/// Named `CodecProfile` rather than `Profile` because `structfs-profiles`
/// uses the latter for capability contracts, which are an unrelated concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodecProfile {
    /// Lossless tagged JSON: the only profile with a canonical spelling.
    ValueJson,
    /// Plain JSON; bytes and non-finite floats are rejected.
    Json,
    /// StructFS CBOR v1.
    Cbor,
    /// StructFS FlexBuffers v1.
    Flexbuffers,
}
impl CodecProfile {
    /// The profile's stable identifier, as used in the specification.
    pub fn identifier(self) -> &'static str {
        match self {
            Self::ValueJson => "structfs-value-json/1",
            Self::Json => "structfs-json/1",
            Self::Cbor => "structfs-cbor/1",
            Self::Flexbuffers => "structfs-flexbuffers/1",
        }
    }
    /// The wire format this profile encodes to and decodes from.
    pub fn format(self) -> Format {
        match self {
            Self::ValueJson => Format::VALUE_JSON,
            Self::Json => Format::JSON,
            Self::Cbor => Format::CBOR,
            Self::Flexbuffers => Format::FLEXBUFFERS,
        }
    }
    /// Whether this profile defines a canonical spelling, i.e. whether
    /// [`ValueCodec::canonical`] accepts it. Only the tagged JSON profile
    /// does: the binary profiles have no whitespace to normalize and plain
    /// JSON has no single spelling to normalize to.
    pub fn has_canonical_form(self) -> bool {
        matches!(self, Self::ValueJson)
    }
}

/// A codec with caller-configured finite limits. Canonical validation applies only
/// to tagged JSON and compares the complete supplied document, including whitespace.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ValueCodec {
    /// The selected wire contract.
    pub profile: CodecProfile,
    /// Finite bounds applied to every encode and decode.
    pub limits: Limits,
    // Not public: it can only be set through `canonical`, which refuses the
    // profiles that have no canonical form. A settable flag would let a
    // caller build a codec whose decode always fails and whose encode
    // silently ignores the request.
    require_canonical: bool,
}
impl ValueCodec {
    /// A codec for `profile` with [`Limits::default`].
    pub fn new(profile: CodecProfile) -> Self {
        Self {
            profile,
            limits: Limits::default(),
            require_canonical: false,
        }
    }
    /// Replace the bounds applied to every encode and decode.
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }
    /// Require the canonical spelling on decode: the supplied document must
    /// be byte-identical to the re-encoding of the value it decodes to,
    /// whitespace included.
    ///
    /// Rejected here, at construction, for any profile without a canonical
    /// form — [`CodecProfile::has_canonical_form`] says which. Deferring the
    /// rejection to `decode` would hand the caller a codec that can never
    /// succeed and an `encode` that quietly ignores the flag.
    ///
    /// ```
    /// use structfs_serde_store::{CodecProfile, ValueCodec};
    ///
    /// assert!(ValueCodec::new(CodecProfile::ValueJson).canonical().is_ok());
    /// assert!(ValueCodec::new(CodecProfile::Cbor).canonical().is_err());
    /// ```
    pub fn canonical(mut self) -> Result<Self, Error> {
        if !self.profile.has_canonical_form() {
            return Err(Error::invalid_argument(format!(
                "profile {} has no canonical form",
                self.profile.identifier()
            )));
        }
        self.require_canonical = true;
        Ok(self)
    }
    /// Whether this codec requires the canonical spelling on decode.
    pub fn requires_canonical(&self) -> bool {
        self.require_canonical
    }
}

/// Validate and transcode a document, even when both profiles are the same.
/// Raw byte forwarding is deliberately a different operation.
pub fn transcode(bytes: &Bytes, source: &ValueCodec, target: &ValueCodec) -> Result<Bytes, Error> {
    let value = source.decode(bytes, &source.profile.format())?;
    target.encode(&value, &target.profile.format())
}
impl Codec for ValueCodec {
    fn supports(&self, format: &Format) -> bool {
        *format == self.profile.format()
    }
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        let result = (|| {
            // `canonical()` already refused every profile without a canonical
            // form, so no runtime profile check is needed here.
            let v = match self.profile {
                CodecProfile::ValueJson => crate::json_profile::decode(bytes, &self.limits, true),
                CodecProfile::Json => crate::json_profile::decode(bytes, &self.limits, false),
                CodecProfile::Cbor => crate::cbor_profile::decode(bytes, &self.limits),
                CodecProfile::Flexbuffers => crate::flex_profile::decode(bytes, &self.limits),
            }?;
            if self.require_canonical
                && crate::json_profile::encode(&v, &self.limits, true)?.as_slice() != bytes.as_ref()
            {
                return Err(Failure::new(CodecErrorKind::Noncanonical));
            }
            Ok(v)
        })();
        result.map_err(|e: Failure| e.core(format, CodecOperation::Decode, &self.limits))
    }
    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        if !self.supports(format) {
            return Err(Error::UnsupportedFormat(format.clone()));
        }
        let result = match self.profile {
            CodecProfile::ValueJson => crate::json_profile::encode(value, &self.limits, true),
            CodecProfile::Json => crate::json_profile::encode(value, &self.limits, false),
            CodecProfile::Cbor => crate::cbor_profile::encode(value, &self.limits),
            CodecProfile::Flexbuffers => crate::flex_profile::encode(value, &self.limits),
        };
        result
            .map(Bytes::from)
            .map_err(|e| e.core(format, CodecOperation::Encode, &self.limits))
    }
}
macro_rules! default_codec {
    ($name:ident,$profile:ident,$doc:literal) => {
        #[doc=$doc]
        #[derive(Debug, Clone, Copy, Default)]
        pub struct $name;
        impl Codec for $name {
            fn supports(&self, f: &Format) -> bool {
                ValueCodec::new(CodecProfile::$profile).supports(f)
            }
            fn decode(&self, b: &Bytes, f: &Format) -> Result<Value, Error> {
                ValueCodec::new(CodecProfile::$profile).decode(b, f)
            }
            fn encode(&self, v: &Value, f: &Format) -> Result<Bytes, Error> {
                ValueCodec::new(CodecProfile::$profile).encode(v, f)
            }
        }
    };
}
default_codec!(
    JsonCodec,
    Json,
    "Strict plain JSON with default limits; bytes and non-finite floats fail."
);
default_codec!(
    ValueJsonCodec,
    ValueJson,
    "Lossless canonical StructFS Value JSON v1 with default limits."
);
default_codec!(CborCodec, Cbor, "StructFS CBOR v1 with default limits.");
default_codec!(
    FlexbuffersCodec,
    Flexbuffers,
    "StructFS FlexBuffers v1 with default limits; NUL map keys fail."
);

/// A codec that combines multiple codecs.
///
/// Routes encode/decode to the appropriate codec based on format.
pub struct MultiCodec {
    codecs: Vec<Box<dyn Codec>>,
}

impl std::fmt::Debug for MultiCodec {
    /// `Codec` is not `Debug`, so the members are reported by the formats
    /// they claim rather than by type.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let supported: Vec<&Format> = [
            &Format::JSON,
            &Format::VALUE_JSON,
            &Format::CBOR,
            &Format::FLEXBUFFERS,
        ]
        .into_iter()
        .filter(|format| self.supports(format))
        .collect();
        f.debug_struct("MultiCodec")
            .field("codecs", &self.codecs.len())
            .field("standard_formats", &supported)
            .finish()
    }
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

    /// The v1 transports, routed by explicit format. Each has its documented subset.
    pub fn standard() -> Self {
        let mut mc = Self::new();
        mc.add(JsonCodec);
        mc.add(CborCodec);
        mc.add(FlexbuffersCodec);
        mc.add(ValueJsonCodec);
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
        assert!(matches!(
            codec.decode(&bytes, &Format::PROTOBUF),
            Err(Error::UnsupportedFormat(_))
        ));
        assert!(matches!(
            codec.encode(&Value::from("test"), &Format::PROTOBUF),
            Err(Error::UnsupportedFormat(_))
        ));
        assert!(codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::PROTOBUF));
        assert!(!codec.supports(&Format::OCTET_STREAM));
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
    fn empty_multi_codec_supports_nothing() {
        let codec = MultiCodec::new();
        assert!(!codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::PROTOBUF));
        assert!(matches!(
            codec.decode(&Bytes::from_static(b"hello"), &Format::JSON),
            Err(Error::UnsupportedFormat(_))
        ));
        assert!(matches!(
            codec.encode(&Value::from("test"), &Format::JSON),
            Err(Error::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn multi_codec_default_is_standard() {
        let codec = MultiCodec::default();
        for format in [
            Format::JSON,
            Format::VALUE_JSON,
            Format::CBOR,
            Format::FLEXBUFFERS,
        ] {
            assert!(codec.supports(&format), "default lacks {format}");
        }
    }

    #[test]
    fn multi_codec_debug_reports_routed_formats() {
        let debug = format!("{:?}", MultiCodec::standard());
        assert!(debug.contains("MultiCodec"), "{debug}");
        assert!(debug.contains("codecs: 4"), "{debug}");
        assert!(format!("{:?}", MultiCodec::new()).contains("codecs: 0"));
    }

    /// Whatever the tagged encoder accepts, the tagged decoder must read
    /// back, however high the caller sets `max_depth`: each semantic level
    /// costs several syntax levels, and the decoder's syntax ceiling is hard.
    #[test]
    fn tagged_json_reads_back_everything_it_writes_up_to_the_ceiling() {
        let codec = ValueCodec::new(CodecProfile::ValueJson)
            .with_limits(Limits::default().with_max_depth(usize::MAX));
        for make in [
            |v: Value| Value::Map([("k".to_string(), v)].into_iter().collect()),
            |v: Value| Value::Array(vec![v]),
        ] {
            let mut value = Value::from(1i64);
            for _ in 0..crate::json_profile::TAGGED_DEPTH_CEILING {
                value = make(value);
            }
            let bytes = codec.encode(&value, &Format::VALUE_JSON).unwrap();
            assert_eq!(codec.decode(&bytes, &Format::VALUE_JSON).unwrap(), value);

            let deeper = make(value);
            assert!(codec.encode(&deeper, &Format::VALUE_JSON).is_err());
        }
    }

    #[test]
    fn canonical_is_rejected_for_profiles_without_one() {
        assert!(ValueCodec::new(CodecProfile::ValueJson)
            .canonical()
            .unwrap()
            .requires_canonical());
        for profile in [
            CodecProfile::Json,
            CodecProfile::Cbor,
            CodecProfile::Flexbuffers,
        ] {
            assert!(!profile.has_canonical_form());
            let err = ValueCodec::new(profile).canonical().unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument { .. }),
                "expected InvalidArgument for {profile:?}, got {err:?}"
            );
        }
        assert!(!ValueCodec::new(CodecProfile::Json).requires_canonical());
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
    fn default_codec_is_copy_default_and_debug() {
        let codec1: JsonCodec = Default::default();
        let codec2 = codec1; // Copy
        let bytes = codec1.encode(&Value::from("test"), &Format::JSON).unwrap();
        assert_eq!(
            codec2.decode(&bytes, &Format::JSON).unwrap(),
            Value::from("test")
        );
        assert!(format!("{:?}", codec1).contains("JsonCodec"));
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
}
