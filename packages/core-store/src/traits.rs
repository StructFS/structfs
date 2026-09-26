//! Core traits: Reader, Writer, Codec.

use bytes::Bytes;

use crate::{Error, Format, Path, Record, Value};

/// Read records from paths.
///
/// This is the semantic read interface. Paths are validated Unicode identifiers,
/// and the returned Record can be either raw bytes or parsed values.
///
/// # Mutability
///
/// Both `Reader::read` and `Writer::write` take `&mut self`. This is intentional:
///
/// 1. **Stateful stores exist**: Some stores maintain state that changes on read.
///    For example:
///    - HTTP broker caches responses after first read
///    - Filesystem store tracks file position
///
/// 2. **Uniformity**: A single trait signature works for all stores. Stores that
///    don't mutate on read simply ignore the mutability—the compiler optimizes
///    this away.
///
/// 3. **No interior mutability tax**: Stores don't need `Mutex` or `RefCell`
///    internally just to satisfy the trait. This avoids runtime overhead and
///    potential deadlocks.
///
/// # Concurrent Access
///
/// For concurrent access to a store, wrap it explicitly:
///
/// ```rust,ignore
/// use std::sync::{Arc, Mutex};
///
/// let store = Arc::new(Mutex::new(MyStore::new()));
///
/// // In thread 1:
/// let mut guard = store.lock().unwrap();
/// guard.read(&path)?;
///
/// // In thread 2:
/// let mut guard = store.lock().unwrap();
/// guard.read(&other_path)?;
/// ```
///
/// This makes synchronization explicit at the usage site rather than hidden
/// in the trait design.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn Reader>`.
pub trait Reader: Send + Sync {
    /// Read a record from a path.
    ///
    /// Returns `Ok(Some(record))` if data exists at the path,
    /// `Ok(None)` if the path doesn't exist,
    /// or `Err` if an error occurred.
    ///
    /// # Routing stores
    ///
    /// A store that routes by prefix (`OverlayStore`, `MountStore`) returns
    /// `Err(Error::NoRoute)` for a path no mounted store covers, and
    /// `Ok(None)` only when the routed store itself reports the path absent.
    /// Layering combinators treat every error as terminal: `Cascade` does
    /// not consult its fallback on `NoRoute`, so mount a catch-all store at
    /// the root when fall-through is wanted.
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error>;

    /// Enumerate one names-only page. Override for bounded large-directory
    /// discovery; the default materializes `read_children` before slicing.
    /// A zero limit or an offset past the end is an `InvalidArgument`
    /// error. Preserve missing versus present-but-empty. `next` must
    /// advance when more names remain.
    fn read_children_page(
        &mut self,
        from: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Option<crate::ChildPage>, Error> {
        if limit == 0 {
            return Err(Error::invalid_argument("child page limit must be positive"));
        }
        let Some(names) = self.read_children(from)? else {
            return Ok(None);
        };
        crate::children::page_names(names, offset, limit).map(Some)
    }

    /// Enumerate the child names directly under a path.
    ///
    /// Returns `Ok(None)` if the path doesn't exist, and `Ok(Some(names))`
    /// otherwise — an empty vec for leaf values.
    ///
    /// The default implementation reads the path and projects children from
    /// the parsed value: map keys, or indices for arrays. Stores that can
    /// enumerate more cheaply should override this; stores that serve
    /// `Record::Raw` *must*, since the default cannot inspect raw bytes and
    /// returns `Error::UnsupportedFormat` with the record's format.
    ///
    /// Listing-style stores (such as `MountStore`'s `ctx/mounts`) may return
    /// multi-segment names like `ctx/sys`, which are readable only when
    /// joined onto `from` as a whole path, not as a single component.
    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        let Some(record) = self.read(from)? else {
            return Ok(None);
        };
        match record.as_value() {
            Some(value) => Ok(Some(crate::children::names_of_value(value))),
            None => Err(Error::UnsupportedFormat(record.format())),
        }
    }
}

/// Write records to paths.
///
/// This is the semantic write interface. Paths are validated Unicode identifiers,
/// and the data can be either raw bytes or parsed values.
///
/// See [`Reader`] for discussion of the `&mut self` requirement.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn Writer>`.
pub trait Writer: Send + Sync {
    /// Write a record to a path.
    ///
    /// Returns the path where data was written. This may differ from the
    /// input path—for example, the HTTP broker returns a handle path like
    /// `/outstanding/0` after queuing a request to the root path.
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error>;
}

/// Combined read/write at the Core level.
pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}

/// Codec for converting between Value and bytes.
///
/// Codecs handle the parsing (decode) and serialization (encode) of data.
/// The Core layer doesn't care about specific formats - that's the codec's job.
///
/// # Implementing Custom Codecs
///
/// ```rust
/// use structfs_core_store::{Codec, Value, Format, Error};
/// use bytes::Bytes;
///
/// struct MyProtobufCodec {
///     // schema, etc.
/// }
///
/// impl Codec for MyProtobufCodec {
///     fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
///         if format != &Format::PROTOBUF {
///             return Err(Error::UnsupportedFormat(format.clone()));
///         }
///         // Parse protobuf bytes into Value...
///         todo!()
///     }
///
///     fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
///         if format != &Format::PROTOBUF {
///             return Err(Error::UnsupportedFormat(format.clone()));
///         }
///         // Serialize Value to protobuf bytes...
///         todo!()
///     }
///
///     fn supports(&self, format: &Format) -> bool {
///         format == &Format::PROTOBUF
///     }
/// }
/// ```
pub trait Codec: Send + Sync {
    /// Decode raw bytes into a Value.
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error>;

    /// Encode a Value into raw bytes.
    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error>;

    /// Check if this codec supports a format.
    fn supports(&self, format: &Format) -> bool;
}

/// A codec that doesn't support any formats.
///
/// Useful as a placeholder or for stores that only deal with parsed Values.
pub struct NoCodec;

impl Codec for NoCodec {
    fn decode(&self, _bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        Err(Error::UnsupportedFormat(format.clone()))
    }

    fn encode(&self, _value: &Value, format: &Format) -> Result<Bytes, Error> {
        Err(Error::UnsupportedFormat(format.clone()))
    }

    fn supports(&self, _format: &Format) -> bool {
        false
    }
}

// Blanket implementations for references and boxes

impl<T: Reader + ?Sized> Reader for &mut T {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        (*self).read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        (*self).read_children(from)
    }
    fn read_children_page(
        &mut self,
        from: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Option<crate::ChildPage>, Error> {
        (*self).read_children_page(from, offset, limit)
    }
}

impl<T: Writer + ?Sized> Writer for &mut T {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        (*self).write(to, data)
    }
}

impl<T: Reader + ?Sized> Reader for Box<T> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        self.as_mut().read(from)
    }

    fn read_children(&mut self, from: &Path) -> Result<Option<Vec<String>>, Error> {
        self.as_mut().read_children(from)
    }
    fn read_children_page(
        &mut self,
        from: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Option<crate::ChildPage>, Error> {
        self.as_mut().read_children_page(from, offset, limit)
    }
}

impl<T: Writer + ?Sized> Writer for Box<T> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        self.as_mut().write(to, data)
    }
}

impl<T: Codec + ?Sized> Codec for Box<T> {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        self.as_ref().decode(bytes, format)
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        self.as_ref().encode(value, format)
    }

    fn supports(&self, format: &Format) -> bool {
        self.as_ref().supports(format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::RawMapStore;
    use crate::{path, MemoryStore};

    #[test]
    fn basic_store_works() {
        let mut store = MemoryStore::new();

        let path = path!("users/123");
        let record = Record::parsed(Value::from("Alice"));

        store.write(&path, record.clone()).unwrap();

        let result = store.read(&path).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn object_safety_works() {
        let mut store = MemoryStore::new();
        let boxed: &mut dyn Store = &mut store;

        let path = path!("test");
        boxed
            .write(&path, Record::parsed(Value::from("hello")))
            .unwrap();

        let result = boxed.read(&path).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn no_codec_decode_fails() {
        let codec = NoCodec;
        let bytes = Bytes::from_static(b"hello");
        let result = codec.decode(&bytes, &Format::JSON);
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn no_codec_encode_fails() {
        let codec = NoCodec;
        let value = Value::from("test");
        let result = codec.encode(&value, &Format::JSON);
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn no_codec_supports_nothing() {
        let codec = NoCodec;
        assert!(!codec.supports(&Format::JSON));
        assert!(!codec.supports(&Format::PROTOBUF));
        assert!(!codec.supports(&Format::OCTET_STREAM));
    }

    #[test]
    fn ref_mut_reader_and_writer_work() {
        let mut store = MemoryStore::new();
        let path = path!("test");

        // Use &mut reference as Writer, then as Reader
        let store_ref: &mut MemoryStore = &mut store;
        store_ref
            .write(&path, Record::parsed(Value::from("value")))
            .unwrap();
        let result = store_ref.read(&path).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn boxed_reader_and_writer_work() {
        let mut boxed: Box<MemoryStore> = Box::new(MemoryStore::new());
        let path = path!("boxed_test");
        boxed
            .write(&path, Record::parsed(Value::from("boxed_value")))
            .unwrap();
        let result = boxed.read(&path).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn boxed_codec_works() {
        // Create a simple test codec
        struct TestCodec;

        impl Codec for TestCodec {
            fn decode(&self, bytes: &Bytes, _format: &Format) -> Result<Value, Error> {
                // Simple: treat bytes as UTF-8 string
                let s = String::from_utf8_lossy(bytes);
                Ok(Value::String(s.to_string()))
            }

            fn encode(&self, value: &Value, _format: &Format) -> Result<Bytes, Error> {
                match value {
                    Value::String(s) => Ok(Bytes::from(s.clone())),
                    _ => Err(Error::encode(Format::OCTET_STREAM, "only strings")),
                }
            }

            fn supports(&self, format: &Format) -> bool {
                format == &Format::OCTET_STREAM
            }
        }

        let boxed: Box<dyn Codec> = Box::new(TestCodec);

        // Test supports
        assert!(boxed.supports(&Format::OCTET_STREAM));
        assert!(!boxed.supports(&Format::JSON));

        // Test decode
        let decoded = boxed
            .decode(&Bytes::from_static(b"hello"), &Format::OCTET_STREAM)
            .unwrap();
        assert_eq!(decoded, Value::String("hello".to_string()));

        // Test encode
        let encoded = boxed
            .encode(&Value::String("world".to_string()), &Format::OCTET_STREAM)
            .unwrap();
        assert_eq!(encoded.as_ref(), b"world");
    }

    #[test]
    fn store_trait_auto_impl() {
        // Verify that anything implementing Reader + Writer auto-implements Store
        fn requires_store<S: Store>(_s: &mut S) {}

        let mut store = MemoryStore::new();
        requires_store(&mut store);
    }

    #[test]
    fn read_missing_returns_none() {
        let mut store = MemoryStore::new();
        let result = store.read(&path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_children_default_impl() {
        use std::collections::BTreeMap;

        // RawMapStore does not override read_children, so this exercises the
        // trait default's value projection.
        let mut store = RawMapStore::new();

        // Map value: children are the keys
        let mut map = BTreeMap::new();
        map.insert("alice".to_string(), Value::from(1i64));
        map.insert("bob".to_string(), Value::from(2i64));
        store
            .write(&path!("users"), Record::parsed(Value::Map(map)))
            .unwrap();
        assert_eq!(
            store.read_children(&path!("users")).unwrap(),
            Some(vec!["alice".to_string(), "bob".to_string()])
        );

        // Array value: children are indices
        store
            .write(
                &path!("items"),
                Record::parsed(Value::Array(vec![Value::from("a"), Value::from("b")])),
            )
            .unwrap();
        assert_eq!(
            store.read_children(&path!("items")).unwrap(),
            Some(vec!["0".to_string(), "1".to_string()])
        );

        // Leaf value: empty children
        store
            .write(&path!("leaf"), Record::parsed(Value::from("scalar")))
            .unwrap();
        assert_eq!(store.read_children(&path!("leaf")).unwrap(), Some(vec![]));

        // Missing path: None
        assert_eq!(store.read_children(&path!("missing")).unwrap(), None);
    }

    #[test]
    fn read_children_page_default_impl() {
        let mut store = RawMapStore::new();
        store
            .write(
                &path!("items"),
                Record::parsed(Value::Array(vec![
                    Value::from("a"),
                    Value::from("b"),
                    Value::from("c"),
                ])),
            )
            .unwrap();
        let page = store
            .read_children_page(&path!("items"), 1, 1)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["1".to_string()]);
        assert_eq!(page.next, Some(2));
        assert!(store
            .read_children_page(&path!("missing"), 0, 1)
            .unwrap()
            .is_none());
        assert!(matches!(
            store.read_children_page(&path!("items"), 0, 0),
            Err(Error::InvalidArgument { .. })
        ));
        assert!(matches!(
            store.read_children_page(&path!("items"), 4, 1),
            Err(Error::InvalidArgument { .. })
        ));
    }

    #[test]
    fn read_children_raw_record_is_unsupported_format() {
        let mut store = RawMapStore::new();
        store
            .write(
                &path!("raw"),
                Record::raw(Bytes::from_static(b"{}"), Format::JSON),
            )
            .unwrap();
        assert!(matches!(
            store.read_children(&path!("raw")),
            Err(Error::UnsupportedFormat(f)) if f == Format::JSON
        ));
    }

    #[test]
    fn read_children_delegates_through_wrappers() {
        /// Store that overrides read_children without storing map values.
        struct ListingStore;

        impl Reader for ListingStore {
            fn read(&mut self, _from: &Path) -> Result<Option<Record>, Error> {
                Ok(None)
            }

            fn read_children(&mut self, _from: &Path) -> Result<Option<Vec<String>>, Error> {
                Ok(Some(vec!["custom".to_string()]))
            }
        }

        let mut store = ListingStore;
        let by_ref: &mut dyn Reader = &mut store;
        assert_eq!(
            by_ref.read_children(&path!("x")).unwrap(),
            Some(vec!["custom".to_string()])
        );

        let mut boxed: Box<dyn Reader> = Box::new(ListingStore);
        assert_eq!(
            boxed.read_children(&path!("x")).unwrap(),
            Some(vec!["custom".to_string()])
        );
        // Paging through the wrapper reaches the override too.
        let page = boxed
            .read_children_page(&path!("x"), 0, 5)
            .unwrap()
            .unwrap();
        assert_eq!(page.names, vec!["custom".to_string()]);
    }
}
