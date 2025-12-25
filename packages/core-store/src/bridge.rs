//! Bridges between LL and Core layers.
//!
//! These adapters allow using LL stores from the Core layer and vice versa.
//!
//! # LL → Core Bridge
//!
//! Wrap an `LLStore` to get a Core `Store`:
//!
//! ```rust,ignore
//! let ll_store = SomeLLStore::new();
//! let core_store = LLToCore::new(ll_store, Format::JSON);
//! // Now use core_store as a Reader/Writer
//! ```
//!
//! # Core → LL Bridge
//!
//! Wrap a Core `Store` to get an `LLStore`:
//!
//! ```rust,ignore
//! let core_store = SomeCoreStore::new();
//! let ll_store = CoreToLL::new(core_store, JsonCodec, Format::JSON);
//! // Now use ll_store as an LLReader/LLWriter
//! ```

use bytes::Bytes;
use structfs_ll_store::{LLError, LLPath, LLReader, LLWriter};

use crate::{Codec, Error, Format, Path, PathError, Reader, Record, Writer};

/// Adapts an LL store to the Core Store interface.
///
/// This bridge:
/// - Converts `&[&[u8]]` paths to validated `Path`
/// - Wraps returned bytes as `Record::Raw` with a format hint
/// - Serializes `Record` to bytes for writes
pub struct LLToCore<T, C> {
    inner: T,
    codec: C,
    /// Format hint for data read from LL layer.
    read_format: Format,
    /// Format to use when serializing for LL writes.
    write_format: Format,
}

impl<T, C> LLToCore<T, C> {
    /// Create a new bridge with the same format for reads and writes.
    pub fn new(inner: T, codec: C, format: Format) -> Self {
        Self {
            inner,
            codec,
            read_format: format.clone(),
            write_format: format,
        }
    }

    /// Create a new bridge with different formats for reads and writes.
    pub fn with_formats(inner: T, codec: C, read_format: Format, write_format: Format) -> Self {
        Self {
            inner,
            codec,
            read_format,
            write_format,
        }
    }

    /// Get a reference to the inner LL store.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner LL store.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Unwrap, returning the inner LL store.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: LLReader, C: Send + Sync> Reader for LLToCore<T, C> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Convert Path to &[&[u8]]
        let components: Vec<&[u8]> = from.components.iter().map(|s| s.as_bytes()).collect();

        // Read via LL
        let bytes = match self.inner.ll_read(&components) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(None),
            Err(e) => return Err(Error::Ll(e)),
        };

        // Wrap as Raw record with our format hint
        Ok(Some(Record::raw(bytes, self.read_format.clone())))
    }
}

impl<T: LLWriter, C: Codec + Send + Sync> Writer for LLToCore<T, C> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // Get bytes from Record (serialize if Parsed)
        let bytes = data.into_bytes(&self.codec, &self.write_format)?;

        // Convert Path to &[&[u8]]
        let components: Vec<&[u8]> = to.components.iter().map(|s| s.as_bytes()).collect();

        // Write via LL
        let result_path = self.inner.ll_write(&components, bytes).map_err(Error::Ll)?;

        // Convert result back to Path
        path_from_ll(&result_path)
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
        // Convert &[&[u8]] to Path
        let path = path_from_bytes(path).map_err(|e| LLError::Protocol {
            code: 1,
            detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
        })?;

        // Read via Core
        let record = match self.inner.read(&path) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => {
                return Err(LLError::Protocol {
                    code: 2,
                    detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
                })
            }
        };

        // Convert to bytes
        let bytes =
            record
                .into_bytes(&self.codec, &self.format)
                .map_err(|e| LLError::Protocol {
                    code: 3,
                    detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
                })?;

        Ok(Some(bytes))
    }
}

impl<T: Writer, C: Send + Sync> LLWriter for CoreToLL<T, C> {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        // Convert path
        let path = path_from_bytes(path).map_err(|e| LLError::Protocol {
            code: 1,
            detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
        })?;

        // Wrap data as Raw record
        let record = Record::raw(data, self.format.clone());

        // Write via Core
        let result_path = self
            .inner
            .write(&path, record)
            .map_err(|e| LLError::Protocol {
                code: 2,
                detail: Bytes::copy_from_slice(e.to_string().as_bytes()),
            })?;

        // Convert result to LL path
        Ok(result_path.to_ll_path())
    }
}

/// Convert LL path components to Core Path.
pub(crate) fn path_from_bytes(components: &[&[u8]]) -> Result<Path, PathError> {
    let mut strings = Vec::with_capacity(components.len());
    for (i, bytes) in components.iter().enumerate() {
        let s = std::str::from_utf8(bytes).map_err(|_| PathError::InvalidComponent {
            component: format!("{:?}", bytes),
            position: i,
            message: "not valid UTF-8".to_string(),
        })?;
        strings.push(s.to_string());
    }
    Path::try_from_components(strings)
}

/// Convert LL path (owned) to Core Path.
pub(crate) fn path_from_ll(components: &[Bytes]) -> Result<Path, Error> {
    let refs: Vec<&[u8]> = components.iter().map(|b| b.as_ref()).collect();
    path_from_bytes(&refs).map_err(Error::Path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{path, NoCodec};
    use std::collections::HashMap;

    /// Simple in-memory LL store for testing.
    struct TestLLStore {
        data: HashMap<Vec<Vec<u8>>, Bytes>,
    }

    impl TestLLStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl LLReader for TestLLStore {
        fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
            let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
            Ok(self.data.get(&key).cloned())
        }
    }

    impl LLWriter for TestLLStore {
        fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
            let key: Vec<Vec<u8>> = path.iter().map(|c| c.to_vec()).collect();
            self.data.insert(key, data);
            Ok(path.iter().map(|c| Bytes::copy_from_slice(c)).collect())
        }
    }

    /// Simple in-memory Core store for testing.
    struct TestCoreStore {
        data: HashMap<Path, Record>,
    }

    impl TestCoreStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Reader for TestCoreStore {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl Writer for TestCoreStore {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[test]
    fn ll_to_core_read() {
        let mut ll = TestLLStore::new();
        ll.data.insert(
            vec![b"users".to_vec(), b"123".to_vec()],
            Bytes::from_static(b"hello"),
        );

        let mut bridge = LLToCore::new(ll, NoCodec, Format::OCTET_STREAM);

        let result = bridge.read(&path!("users/123")).unwrap();
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().as_bytes(),
            Some(&Bytes::from_static(b"hello"))
        );
    }

    #[test]
    fn ll_to_core_write() {
        let ll = TestLLStore::new();
        let mut bridge = LLToCore::new(ll, NoCodec, Format::OCTET_STREAM);

        let record = Record::raw(Bytes::from_static(b"data"), Format::OCTET_STREAM);
        bridge.write(&path!("test/path"), record).unwrap();

        // Verify it was written
        let key = vec![b"test".to_vec(), b"path".to_vec()];
        assert!(bridge.inner().data.contains_key(&key));
    }

    #[test]
    fn core_to_ll_read() {
        let mut core = TestCoreStore::new();
        core.data.insert(
            path!("users/123"),
            Record::raw(Bytes::from_static(b"hello"), Format::OCTET_STREAM),
        );

        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        let result = bridge.ll_read(&[b"users", b"123"]).unwrap();
        assert_eq!(result, Some(Bytes::from_static(b"hello")));
    }

    #[test]
    fn core_to_ll_write() {
        let core = TestCoreStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        bridge
            .ll_write(&[b"test", b"path"], Bytes::from_static(b"data"))
            .unwrap();

        // Verify it was written
        assert!(bridge.inner().data.contains_key(&path!("test/path")));
    }

    #[test]
    fn invalid_utf8_path_rejected() {
        let core = TestCoreStore::new();
        let mut bridge = CoreToLL::new(core, NoCodec, Format::OCTET_STREAM);

        // Invalid UTF-8 sequence
        let result = bridge.ll_read(&[&[0xFF, 0xFE]]);
        assert!(matches!(result, Err(LLError::Protocol { .. })));
    }
}
