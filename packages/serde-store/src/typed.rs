//! Typed reader and writer extension traits.
//!
//! See the [crate docs](crate#typed-access) for the operation matrix these
//! share with the async and detached flavours, and for why every
//! codec-taking method takes an `Arc<dyn Codec>`.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;

use structfs_core_store::{Codec, Error, Path, Reader, Record, Writer};

use crate::convert::{from_value, to_value};

/// Extension trait for typed reads.
///
/// This trait is automatically implemented for all `Reader` implementations.
/// It provides convenience methods for reading data directly into Rust types.
///
/// # Example
///
/// ```rust
/// use std::sync::Arc;
/// use serde::Deserialize;
/// use structfs_core_store::{path, Codec, Error, MemoryStore, Record, Value, Writer};
/// use structfs_serde_store::{JsonCodec, TypedReader, TypedWriter};
///
/// #[derive(Debug, PartialEq, Deserialize, serde::Serialize)]
/// struct Config {
///     debug: bool,
///     port: u16,
/// }
///
/// let mut store = MemoryStore::new();
/// store.write_typed(&path!("config"), &Config { debug: true, port: 8080 })?;
///
/// // Parsed records need no codec.
/// let config: Config = store.read_typed(&path!("config"))?.unwrap();
/// assert_eq!(config, Config { debug: true, port: 8080 });
///
/// // Raw records do.
/// let codec: Arc<dyn Codec> = Arc::new(JsonCodec);
/// let config: Option<Config> = store.read_as(&path!("config"), codec)?;
/// assert!(config.is_some());
/// # Ok::<(), Error>(())
/// ```
pub trait TypedReader: Reader {
    /// Read a value and deserialize it into a Rust type.
    ///
    /// This method:
    /// 1. Reads the Record from the store
    /// 2. Parses it to a Value using the codec (if raw)
    /// 3. Deserializes the Value to the target type
    fn read_as<T: DeserializeOwned>(
        &mut self,
        from: &Path,
        codec: Arc<dyn Codec>,
    ) -> Result<Option<T>, Error> {
        let Some(record) = self.read(from)? else {
            return Ok(None);
        };

        let value = record.into_value(codec.as_ref())?;
        let typed = from_value(value)?;
        Ok(Some(typed))
    }

    /// Parsed-only typed read. Raw records require an explicit codec via `read_as`.
    fn read_typed<T: DeserializeOwned>(&mut self, from: &Path) -> Result<Option<T>, Error> {
        let Some(record) = self.read(from)? else {
            return Ok(None);
        };
        Ok(Some(from_value(
            record.into_value(&structfs_core_store::NoCodec)?,
        )?))
    }

    /// Enumerate children at a prefix and deserialize each into `T`.
    ///
    /// Returns pairs of `(child_name, value)`. Children that are absent
    /// between the enumeration and the read are skipped.
    fn read_children_typed<T: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<Vec<(String, T)>>, Error> {
        let Some(children) = self.read_children(from)? else {
            return Ok(None);
        };
        let mut result = Vec::with_capacity(children.len());
        for name in children {
            let child_path = from.child(structfs_core_store::PathComponent::try_new(&name)?);
            if let Some(value) = self.read_typed(&child_path)? {
                result.push((name, value));
            }
        }
        Ok(Some(result))
    }
}

// Blanket implementation for all Readers
impl<R: Reader + ?Sized> TypedReader for R {}

/// Extension trait for typed writes.
///
/// This trait is automatically implemented for all `Writer` implementations.
/// It provides convenience methods for writing Rust types directly.
///
/// There is no codec-taking counterpart to [`TypedReader::read_as`]: a typed
/// write always produces a parsed record, and it is the store — not the
/// caller — that decides whether and how to serialize it.
///
/// # Example
///
/// ```rust
/// use serde::Serialize;
/// use structfs_core_store::{path, Error, MemoryStore};
/// use structfs_serde_store::TypedWriter;
///
/// #[derive(Serialize)]
/// struct User {
///     name: String,
///     email: String,
/// }
///
/// let mut store = MemoryStore::new();
/// let at = store.write_typed(
///     &path!("users/new"),
///     &User { name: "Ada".into(), email: "ada@example.com".into() },
/// )?;
/// assert_eq!(at.to_string(), "users/new");
/// # Ok::<(), Error>(())
/// ```
pub trait TypedWriter: Writer {
    /// Serialize a Rust type and write it to the store.
    ///
    /// This method:
    /// 1. Serializes the data to a Value
    /// 2. Wraps it in a Record::Parsed
    /// 3. Writes it to the store
    fn write_typed<T: Serialize + ?Sized>(&mut self, to: &Path, data: &T) -> Result<Path, Error> {
        let value = to_value(data)?;
        self.write(to, Record::parsed(value))
    }
}

// Blanket implementation for all Writers
impl<W: Writer + ?Sized> TypedWriter for W {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use structfs_core_store::{path, Format, MemoryStore};

    /// The type the sync, async and detached typed tests all round-trip, so
    /// the three flavours are checked against the same shape.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    pub(crate) struct TestUser {
        pub name: String,
        pub age: u32,
    }

    pub(crate) fn alice() -> TestUser {
        TestUser {
            name: "Alice".to_string(),
            age: 30,
        }
    }

    /// A raw JSON record of [`alice`], for exercising the codec path.
    pub(crate) fn raw_alice() -> Record {
        Record::raw(
            crate::Bytes::from_static(b"{\"name\":\"Alice\",\"age\":30}"),
            Format::JSON,
        )
    }

    /// A store that keeps records verbatim. `MemoryStore` parses on write,
    /// so it cannot hold a raw record; the codec-path tests need one that
    /// can. Shared with the async and detached flavours' tests so there is
    /// one such store in the crate rather than one per module.
    #[derive(Default)]
    pub(crate) struct RecordStore {
        data: std::collections::HashMap<Path, Record>,
    }

    impl Reader for RecordStore {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            Ok(self.data.get(from).cloned())
        }
    }

    impl Writer for RecordStore {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            self.data.insert(to.clone(), data);
            Ok(to.clone())
        }
    }

    #[test]
    fn typed_roundtrip_is_codec_free() {
        let mut store = MemoryStore::new();
        store.write_typed(&path!("users/alice"), &alice()).unwrap();

        let recovered: TestUser = store.read_typed(&path!("users/alice")).unwrap().unwrap();
        assert_eq!(recovered, alice());

        let missing: Option<TestUser> = store.read_typed(&path!("nope")).unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn read_as_supplies_the_codec_raw_records_need() {
        let mut store = RecordStore::default();
        store.write(&path!("raw"), raw_alice()).unwrap();

        // Codec-free reads cannot parse a raw record.
        assert!(store.read_typed::<TestUser>(&path!("raw")).is_err());

        let codec: Arc<dyn Codec> = Arc::new(crate::JsonCodec);
        let user: TestUser = store
            .read_as(&path!("raw"), codec.clone())
            .unwrap()
            .unwrap();
        assert_eq!(user, alice());

        // A missing path is None, not an error, even with a codec.
        let missing: Option<TestUser> = store.read_as(&path!("nonexistent"), codec).unwrap();
        assert!(missing.is_none());
    }

    #[test]
    fn read_typed_rejects_non_json_raw() {
        let mut store = RecordStore::default();
        store
            .write(
                &path!("raw"),
                Record::raw(crate::Bytes::from_static(b"data"), Format::OCTET_STREAM),
            )
            .unwrap();

        let result: Result<Option<TestUser>, _> = store.read_typed(&path!("raw"));
        assert!(matches!(result, Err(Error::UnsupportedFormat(_))));
    }

    #[test]
    fn read_children_typed_works() {
        let mut store = MemoryStore::new();
        store.write_typed(&path!("users/alice"), &alice()).unwrap();
        store
            .write_typed(
                &path!("users/bob"),
                &TestUser {
                    name: "Bob".to_string(),
                    age: 40,
                },
            )
            .unwrap();

        let users: Vec<(String, TestUser)> =
            store.read_children_typed(&path!("users")).unwrap().unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].0, "alice");
        assert_eq!(users[0].1, alice());
        assert_eq!(users[1].0, "bob");

        let missing: Option<Vec<(String, TestUser)>> =
            store.read_children_typed(&path!("nowhere")).unwrap();
        assert!(missing.is_none());
    }
}
