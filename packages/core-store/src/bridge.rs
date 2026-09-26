//! Bridge from the Core layer down to the LL layer.
//!
//! [`CoreToLL`] wraps a Core `Store` so it can be driven through the
//! byte-only `LLReader`/`LLWriter` interface — the shape a wasm or wire
//! boundary speaks:
//!
//! ```rust,ignore
//! let core_store = SomeCoreStore::new();
//! let ll_store = CoreToLL::new(core_store, JsonCodec, Format::JSON);
//! // Now use ll_store as an LLReader/LLWriter
//! ```
//!
//! Core errors crossing the bridge become `LLError::Protocol` with one of
//! the [`protocol`] codes.

use bytes::Bytes;
use structfs_ll_store::{LLError, LLPath, LLReader, LLWriter};

use crate::{Codec, Error, Format, Path, Reader, Record, Writer};

/// `LLError::Protocol` codes emitted by [`CoreToLL`].
pub mod protocol {
    /// The byte path is not a valid `Path` (non-UTF-8 or bad component).
    pub const INVALID_PATH: u32 = 1;
    /// The wrapped Core store returned an error.
    pub const STORE_ERROR: u32 = 2;
    /// The record could not be encoded into the bridge's format.
    pub const ENCODE_ERROR: u32 = 3;
}

/// Wrap any displayable error as an `LLError::Protocol` with `code`.
fn protocol_error(code: u32, error: impl std::fmt::Display) -> LLError {
    LLError::Protocol {
        code,
        detail: Bytes::from(error.to_string()),
    }
}

/// Adapts a Core store to the LL Store interface.
///
/// This bridge:
/// - Converts `&[&[u8]]` paths to validated `Path`
/// - Parses/serializes data as needed
/// - Returns bytes in the configured format
pub struct CoreToLL<T, C> {
    inner: T,
    codec: C,
    format: Format,
}

impl<T, C> CoreToLL<T, C> {
    /// Create a new bridge.
    pub fn new(inner: T, codec: C, format: Format) -> Self {
        Self {
            inner,
            codec,
            format,
        }
    }

    /// Get a reference to the inner Core store.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner Core store.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Unwrap, returning the inner Core store.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Reader, C: Codec + Send + Sync> LLReader for CoreToLL<T, C> {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        let path = path_from_bytes(path).map_err(|e| protocol_error(protocol::INVALID_PATH, e))?;

        let record = match self.inner.read(&path) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => return Err(protocol_error(protocol::STORE_ERROR, e)),
        };

        record
            .into_bytes(&self.codec, &self.format)
            .map(Some)
            .map_err(|e| protocol_error(protocol::ENCODE_ERROR, e))
    }
}

impl<T: Writer, C: Send + Sync> LLWriter for CoreToLL<T, C> {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        let path = path_from_bytes(path).map_err(|e| protocol_error(protocol::INVALID_PATH, e))?;

        let record = Record::raw(data, self.format.clone());

        let result_path = self
            .inner
            .write(&path, record)
            .map_err(|e| protocol_error(protocol::STORE_ERROR, e))?;

        // Widen the validated result path to LL (free — no component copy).
        Ok(result_path.into_ll())
    }
}

/// Convert borrowed LL path components to a validated Core `Path`.
///
/// One copy (borrowed slices into owned `Bytes`), then the single
/// narrowing point `Path::validate` — no intermediate `String`s.
pub(crate) fn path_from_bytes(components: &[&[u8]]) -> Result<Path, Error> {
    let ll: LLPath = components
        .iter()
        .map(|b| Bytes::copy_from_slice(b))
        .collect();
    Path::validate(ll).map_err(Error::Path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RawMapStore;
    use crate::{path, NoCodec};

    #[test]
    fn core_to_ll_read() {
        let mut core = RawMapStore::new();
        core.insert(
            path!("users/123"),
            Record::raw(Bytes::from_static(b"hello"), Format::OCTET_STREAM),
        );

        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge.ll_read(&[b"users", b"123"]).unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"hello")));
    }

    #[test]
    fn core_to_ll_write() {
        let core = RawMapStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge
            .ll_write(&[b"test", b"path"], Bytes::from_static(b"data"))
            .unwrap();
        assert_eq!(result.len(), 2);

        // Verify it was written
        assert!(bridge.inner().contains(&path!("test/path")));
    }

    #[test]
    fn invalid_utf8_path_rejected() {
        let core = RawMapStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        // Invalid UTF-8 sequence
        let result = bridge.ll_read(&[&[0xFF, 0xFE]]);
        assert!(matches!(
            result,
            Err(LLError::Protocol {
                code: protocol::INVALID_PATH,
                ..
            })
        ));
    }

    #[test]
    fn core_to_ll_inner_methods() {
        let core = RawMapStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        // Test inner()
        assert!(!bridge.inner().contains(&path!("key")));

        // Test inner_mut()
        bridge.inner_mut().insert(
            path!("key"),
            Record::raw(Bytes::from_static(b"value"), Format::OCTET_STREAM),
        );
        assert!(bridge.inner().contains(&path!("key")));

        // Test into_inner()
        let core = bridge.into_inner();
        assert!(core.contains(&path!("key")));
    }

    #[test]
    fn core_to_ll_read_none() {
        let core = RawMapStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge.ll_read(&[b"nonexistent"]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn core_to_ll_write_invalid_utf8() {
        let core = RawMapStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        // Invalid UTF-8 sequence
        let result = bridge.ll_write(&[&[0xFF, 0xFE]], Bytes::from_static(b"data"));
        assert!(matches!(
            result,
            Err(LLError::Protocol {
                code: protocol::INVALID_PATH,
                ..
            })
        ));
    }

    #[test]
    fn core_to_ll_encode_error() {
        // A parsed record with NoCodec cannot be encoded for the wire.
        let mut core = RawMapStore::new();
        core.insert(path!("parsed"), Record::parsed(crate::Value::from(1i64)));
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);
        assert!(matches!(
            bridge.ll_read(&[b"parsed"]),
            Err(LLError::Protocol {
                code: protocol::ENCODE_ERROR,
                ..
            })
        ));
    }

    #[test]
    fn path_from_bytes_empty() {
        let result = path_from_bytes(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn path_from_bytes_single_component() {
        let result = path_from_bytes(&[b"users"]).unwrap();
        assert_eq!(result.to_string(), "users");
    }

    #[test]
    fn path_from_bytes_multiple_components() {
        let result = path_from_bytes(&[b"users", b"123", b"profile"]).unwrap();
        assert_eq!(result.to_string(), "users/123/profile");
    }

    #[test]
    fn path_from_bytes_invalid_utf8() {
        let result = path_from_bytes(&[&[0xFF, 0xFE]]);
        assert!(matches!(result, Err(Error::Path(_))));
    }

    /// Store that always returns an error on read.
    struct ErrorCoreStore;

    impl Reader for ErrorCoreStore {
        fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
            Err(Error::store("test", "read", "read error"))
        }
    }

    impl Writer for ErrorCoreStore {
        fn write(&mut self, _to: &Path, _data: Record) -> Result<Path, Error> {
            Err(Error::store("test", "write", "write error"))
        }
    }

    #[test]
    fn core_to_ll_read_error() {
        let core = ErrorCoreStore;
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge.ll_read(&[b"any"]);
        assert!(matches!(
            result,
            Err(LLError::Protocol {
                code: protocol::STORE_ERROR,
                ..
            })
        ));
    }

    #[test]
    fn core_to_ll_write_error() {
        let core = ErrorCoreStore;
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge.ll_write(&[b"any"], Bytes::from_static(b"data"));
        match result {
            Err(LLError::Protocol { code, detail }) => {
                assert_eq!(code, protocol::STORE_ERROR);
                assert!(std::str::from_utf8(&detail)
                    .unwrap()
                    .contains("write error"));
            }
            other => panic!("expected protocol error, got {:?}", other),
        }
    }
}
