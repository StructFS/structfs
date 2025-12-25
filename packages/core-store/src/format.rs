//! Format hints for wire encoding.

use std::borrow::Cow;
use std::fmt;

/// A hint about the wire format of raw bytes.
///
/// Format is used to guide codecs when parsing `Record::Raw` bytes into
/// `Value`, or when serializing `Value` back to bytes.
///
/// This uses MIME-type-like strings for familiarity, but you can use
/// any string that your codecs understand.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Format(pub Cow<'static, str>);

impl Format {
    // Common formats as constants for efficiency

    /// JSON format (`application/json`)
    pub const JSON: Format = Format(Cow::Borrowed("application/json"));

    /// Protocol Buffers (`application/protobuf`)
    pub const PROTOBUF: Format = Format(Cow::Borrowed("application/protobuf"));

    /// MessagePack (`application/msgpack`)
    pub const MSGPACK: Format = Format(Cow::Borrowed("application/msgpack"));

    /// CBOR (`application/cbor`)
    pub const CBOR: Format = Format(Cow::Borrowed("application/cbor"));

    /// Opaque binary data (`application/octet-stream`)
    pub const OCTET_STREAM: Format = Format(Cow::Borrowed("application/octet-stream"));

    /// A parsed Value that was never serialized.
    ///
    /// Used when a Record contains a Value that hasn't been through a codec.
    pub const VALUE: Format = Format(Cow::Borrowed("application/x-structfs-value"));

    /// Create a format from a static string.
    pub const fn from_static(s: &'static str) -> Self {
        Format(Cow::Borrowed(s))
    }

    /// Create a format from an owned string.
    pub fn new(s: impl Into<String>) -> Self {
        Format(Cow::Owned(s.into()))
    }

    /// Get the format string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Check if this is JSON format.
    pub fn is_json(&self) -> bool {
        self == &Self::JSON
    }

    /// Check if this is protobuf format.
    pub fn is_protobuf(&self) -> bool {
        self == &Self::PROTOBUF
    }

    /// Check if this is the VALUE format (parsed, never serialized).
    pub fn is_value(&self) -> bool {
        self == &Self::VALUE
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&'static str> for Format {
    fn from(s: &'static str) -> Self {
        Format(Cow::Borrowed(s))
    }
}

impl From<String> for Format {
    fn from(s: String) -> Self {
        Format(Cow::Owned(s))
    }
}

impl AsRef<str> for Format {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_work() {
        assert_eq!(Format::JSON.as_str(), "application/json");
        assert!(Format::JSON.is_json());
        assert!(!Format::JSON.is_protobuf());
    }

    #[test]
    fn custom_formats() {
        let f = Format::new("application/x-custom");
        assert_eq!(f.as_str(), "application/x-custom");
    }

    #[test]
    fn equality() {
        assert_eq!(Format::JSON, Format::from("application/json"));
        assert_eq!(Format::JSON, Format::new("application/json".to_string()));
    }
}
