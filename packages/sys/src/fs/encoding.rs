//! Open modes, content encodings, and request-field parsing for `fs`.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Value};

/// File open mode, selected by the `mode` field of an `open` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum OpenMode {
    /// `read`: open an existing file for reading.
    #[default]
    Read,
    /// `write`: create or truncate, then write.
    Write,
    /// `append`: create if missing, write at the end.
    Append,
    /// `readwrite`: create if missing, read and write without truncating.
    ReadWrite,
    /// `create_new` (alias `createnew`): create a file that must not exist.
    CreateNew,
}

impl OpenMode {
    /// Every accepted spelling, canonical name first per mode.
    pub(crate) const NAMES: &'static [&'static str] =
        &["read", "write", "append", "readwrite", "create_new"];

    /// Parse a mode name; unknown names are `InvalidArgument`.
    pub fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "append" => Ok(Self::Append),
            "readwrite" => Ok(Self::ReadWrite),
            "create_new" | "createnew" => Ok(Self::CreateNew),
            other => Err(Error::invalid_argument(format!(
                "unknown open mode '{other}' (expected one of: {})",
                Self::NAMES.join(", ")
            ))),
        }
    }

    /// The canonical name of this mode.
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Append => "append",
            Self::ReadWrite => "readwrite",
            Self::CreateNew => "create_new",
        }
    }
}

/// How file content is represented in values, selected by the `encoding`
/// field of an `open` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ContentEncoding {
    /// `base64` (default): content is a base64 string.
    #[default]
    Base64,
    /// `utf8` (aliases `utf-8`, `text`): content is a UTF-8 string; reads
    /// of invalid UTF-8 fail.
    Utf8,
    /// `bytes` (alias `raw`): content is `Value::Bytes`.
    Bytes,
}

impl ContentEncoding {
    pub(crate) const NAMES: &'static [&'static str] = &["base64", "utf8", "bytes"];

    /// Parse an encoding name (case-insensitive); unknown names are
    /// `InvalidArgument`.
    pub fn parse(name: &str) -> Result<Self, Error> {
        match name.to_ascii_lowercase().as_str() {
            "base64" => Ok(Self::Base64),
            "utf8" | "utf-8" | "text" => Ok(Self::Utf8),
            "bytes" | "raw" => Ok(Self::Bytes),
            other => Err(Error::invalid_argument(format!(
                "unknown encoding '{other}' (expected one of: {})",
                Self::NAMES.join(", ")
            ))),
        }
    }

    /// The canonical name of this encoding.
    pub fn name(self) -> &'static str {
        match self {
            Self::Base64 => "base64",
            Self::Utf8 => "utf8",
            Self::Bytes => "bytes",
        }
    }

    /// Represent bytes read from a file.
    pub(crate) fn encode(self, buffer: Vec<u8>) -> Result<Value, Error> {
        match self {
            Self::Base64 => Ok(Value::String(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &buffer,
            ))),
            Self::Utf8 => String::from_utf8(buffer)
                .map(Value::String)
                .map_err(|e| Error::invalid_argument(format!("file content is not UTF-8: {e}"))),
            Self::Bytes => Ok(Value::Bytes(buffer)),
        }
    }

    /// Turn a written value into bytes. `Value::Bytes` is always accepted;
    /// strings are base64-decoded under `base64` and taken verbatim otherwise.
    pub(crate) fn decode(self, value: &Value) -> Result<Vec<u8>, Error> {
        match value {
            Value::String(s) => match self {
                Self::Base64 => {
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                        .map_err(|e| Error::invalid_argument(format!("invalid base64: {e}")))
                }
                Self::Utf8 | Self::Bytes => Ok(s.as_bytes().to_vec()),
            },
            Value::Bytes(b) => Ok(b.to_vec()),
            _ => Err(Error::invalid_argument(
                "file content must be a string or bytes",
            )),
        }
    }
}

/// The request map of an `fs` action.
pub(crate) fn request_map<'a>(
    value: &'a Value,
    action: &str,
) -> Result<&'a BTreeMap<String, Value>, Error> {
    match value {
        Value::Map(map) => Ok(map),
        _ => Err(Error::invalid_argument(format!(
            "{action} requires a map argument"
        ))),
    }
}

/// A required string field of a request map.
pub(crate) fn required_str<'a>(
    map: &'a BTreeMap<String, Value>,
    field: &str,
    action: &str,
) -> Result<&'a str, Error> {
    match map.get(field) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(Error::invalid_argument(format!(
            "{action}: '{field}' must be a string"
        ))),
        None => Err(Error::invalid_argument(format!(
            "{action} requires a '{field}' field"
        ))),
    }
}

/// An optional string field of a request map.
pub(crate) fn optional_str<'a>(
    map: &'a BTreeMap<String, Value>,
    field: &str,
    action: &str,
) -> Result<Option<&'a str>, Error> {
    match map.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(Error::invalid_argument(format!(
            "{action}: '{field}' must be a string"
        ))),
    }
}

/// A byte offset or count: a non-negative integer. Negative values are
/// `InvalidArgument` instead of wrapping through `as u64`.
pub(crate) fn non_negative(value: &Value, what: &str) -> Result<u64, Error> {
    match value {
        Value::Integer(n) => u64::try_from(*n)
            .map_err(|_| Error::invalid_argument(format!("{what} must not be negative (got {n})"))),
        Value::Unsigned(n) => Ok(*n),
        _ => Err(Error::invalid_argument(format!(
            "{what} must be a non-negative integer"
        ))),
    }
}
