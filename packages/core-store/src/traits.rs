//! Core traits: Reader, Writer, Codec.

use bytes::Bytes;

use crate::{Error, Format, Path, Record, Value};

/// Read records from paths.
///
/// This is the semantic read interface. Paths are validated Unicode identifiers,
/// and the returned Record can be either raw bytes or parsed values.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn Reader>`.
pub trait Reader: Send + Sync {
    /// Read a record from a path.
    ///
    /// # Returns
    ///
    /// * `Ok(None)` - The path does not exist.
    /// * `Ok(Some(record))` - The record at the path.
    /// * `Err(Error)` - An error occurred.
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error>;
}

/// Write records to paths.
///
/// This is the semantic write interface. Paths are validated Unicode identifiers,
/// and the data can be either raw bytes or parsed values.
///
/// # Object Safety
///
/// This trait is object-safe: you can use `Box<dyn Writer>`.
pub trait Writer: Send + Sync {
    /// Write a record to a path.
    ///
    /// # Returns
    ///
    /// The "result path". This may be:
    /// - The same as the input path (for simple stores)
    /// - A different path (e.g., a generated ID, a handle for async operations)
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
    use std::collections::HashMap;

    /// Simple in-memory store for testing.
    struct TestStore {
        data: HashMap<Path, Record>,
    }

    impl TestStore {
        fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Reader for TestStore {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl Writer for TestStore {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[test]
    fn basic_store_works() {
        use crate::path;

        let mut store = TestStore::new();

        let path = path!("users/123");
        let record = Record::parsed(Value::from("Alice"));

        store.write(&path, record.clone()).unwrap();

        let result = store.read(&path).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn object_safety_works() {
        use crate::path;

        let mut store = TestStore::new();
        let boxed: &mut dyn Store = &mut store;

        let path = path!("test");
        boxed
            .write(&path, Record::parsed(Value::from("hello")))
            .unwrap();

        let result = boxed.read(&path).unwrap();
        assert!(result.is_some());
    }
}
