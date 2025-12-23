use std::io;
use std::sync;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;

pub use crate::path::{Error as PathError, Path};

/// A `Capability` represents access to a given resource
///
/// A capability may expire at any time.  See the origin of any given
/// capability for a description of under what circumstances that expiration
/// may occur.
pub struct Capability<AuthorityT> {
    // TODO(alex): Define/implement `authority`.  This should allow proper authorization for
    // accessing the Capability.
    pub authority: AuthorityT,
    pub location: Path,
}

// TODO(akesling): Figure out how to do a "reserved symbol" namespace that signals to deserializers
// that there's a specialized object that should be read a particular way.
//
// Or maybe this should just be delegated to the reader to parse things into a desired shape and
// then run something like a "dereferencing" pass over it....

/// A `Reference` represents a pointer to a resource
///
/// References may be used by a given Store to implement pagination,
/// lazy reading of a large resource, or access to a remote resource.
/// Consider recursively listing a sub-tree representing a file-system:
/// it would be undesirable to read the full contents of all files being
/// listed if it isn't desired by the caller.  If such a system returned
/// a reference in place of the data for listed files, it allows separate
/// _intentional_ request of the data at the client's request.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Reference {
    pub location: Path,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum Error {
    #[error("{0}")]
    PathError(#[from] PathError),

    #[error("An error occurred while serializing a record: {message}")]
    RecordSerialization { message: String },
    #[error("An error occurred while deserializing a record: {message}")]
    RecordDeserialization { message: String },
    #[error("An implementation error occurred: {message}")]
    ImplementationFailure { message: String },
    #[error("Error: {message}")]
    Raw { message: String },
    #[error("An unknown error occurred: {message}")]
    Unknown { message: String },
}

pub trait Writer {
    // TODO(alex): Make a record method like `doc()` in Firestore which allows direct record
    // management instead of having to juggle the Store implementation everywhere.

    /// Writes `data` to `destination` within the store.
    ///
    /// Note that the path written to must be compatible with the existing tree up
    /// until the last path component.  I.e. the parent of the new data must be a
    /// compatible structure where that path can be written as a child.
    ///
    /// Example: writing `"foo"` at path `bar/baz/qux` into
    /// ```json
    /// {
    ///     "bar": "baz"
    /// }
    /// ```
    /// will result in an error as the string `"baz"` is not a valid parent for
    /// the key/value pair `"qux": "foo"`.
    ///
    /// To achieve the likely desired result, either the write should be broken
    /// down to multiple steps building up the tree, or a "larger" structure
    /// should be written "higher up" in the record hierarchy: e.g. write
    /// `{"qux": "foo"}` at `bar/baz` instead (this works as the structure at
    /// `"bar"` supports key/value children).
    ///
    /// The returned `Path` represents the "result" of the written record.
    /// Different `Store` backends may have different meanings for a result
    /// `Path`.  A simple in-memory store might just return the same path as
    /// that provided, while a synthetic store embedding a client handling
    /// an HTTP POST request may return the `Path` to find the response.
    // TODO(alex): Switch to accepting `data` by reference as not all stores make use of owned
    // values.
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, Error>;
}

impl<T: Writer> Writer for &mut T {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        (*self).write(destination, data)
    }
}

impl<T: Writer> Writer for sync::Arc<T> {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        sync::Arc::get_mut(self)
            .ok_or_else(|| Error::ImplementationFailure {
                message: concat!(
                    "Getting T for Arc<T> implementing Writer::write ",
                    "resulted in None."
                )
                .to_string(),
            })?
            .write(destination, data)
    }
}

#[async_trait::async_trait]
pub trait AsyncWriter {
    async fn write<RecordType: Serialize + Send>(
        &mut self,
        destination: Path,
        data: RecordType,
    ) -> Result<Path, Error>;
}

pub(crate) trait ObjectSafeWriter {
    fn object_safe_write(
        &mut self,
        to: &Path,
        data: &dyn erased_serde::Serialize,
    ) -> Result<Path, Error>;
}

// impl dyn ObjectSafeWriter {
//     pub(crate) fn erase<'sw, SW: Writer + 'sw>(store_writer: SW) -> WriterWrapper<SW> {
//         WriterWrapper {
//             inner: Some(store_writer),
//         }
//     }
// }

impl Writer for dyn ObjectSafeWriter {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, Error> {
        self.object_safe_write(destination, &data)
    }
}

pub(crate) struct WriterWrapper<S> {
    inner: Option<S>,
}

impl<S> WriterWrapper<S> {
    pub(crate) fn as_mut(&mut self) -> &mut S {
        self.inner.as_mut().unwrap()
    }
}

impl<S: Writer> ObjectSafeWriter for WriterWrapper<S> {
    fn object_safe_write(
        &mut self,
        to: &Path,
        data: &dyn erased_serde::Serialize,
    ) -> Result<Path, Error> {
        self.as_mut().write(to, data)
    }
}

pub trait Reader {
    // TODO(alex): When GATs are supported, constrain this to implement `serde::Deserializer`.

    /// The type implementing a `serde::Deserializer` that will "unpack" the read value.
    ///
    /// By providing both serde::Deserializer and serde::Deserialize variants of read, the caller
    /// can decide how best to consume the deserializing type.  Among other things, this enables
    /// OverlayStore to encapsulate type erasure for deserialization without it leaking into the
    /// public interface.
    ///
    /// This currently requires `erased_serde` in the public interface as the GAT solution proved...
    /// difficult and it is unclear when GATs will become stable.
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de;

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error>;
}

impl<T: Reader> Reader for sync::Arc<T> {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
    where
        'this: 'de,
    {
        sync::Arc::get_mut(self)
            .ok_or_else(|| Error::ImplementationFailure {
                message: concat!(
                    "Getting T for Arc<T> implementing Reader::read_to_deserializer ",
                    "resulted in None."
                )
                .to_string(),
            })?
            .read_to_deserializer(from)
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, Error> {
        sync::Arc::get_mut(self)
            .ok_or_else(|| Error::ImplementationFailure {
                message: "Getting T for Arc<T> implementing Reader::read_owned resulted in None."
                    .to_string(),
            })?
            .read_owned(from)
    }
}

#[async_trait::async_trait]
pub trait AsyncReader {
    async fn read_to_deserializer(
        &mut self,
        from: Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'static> + Send>>, Error>;

    async fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: Path,
    ) -> Result<Option<RecordType>, Error>;
}

// TODO(alex): Create intermediate type which does the same trick as ObjectSafeWriter above and
// intermediates a generic callback argument for this store read which, in turn, can be used to
// capture the generic return needed in the actual Reader implementation....
pub(crate) trait ObjectSafeReader {
    fn object_safe_read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
        callback: &mut dyn FnMut(
            Option<Box<dyn erased_serde::Deserializer<'de>>>,
        ) -> Result<(), Error>,
    ) -> Result<(), Error>
    where
        'this: 'de;
}

// impl dyn ObjectSafeReader {
//     pub(crate) fn erase<SR: Reader>(store_reader: SR) -> ReaderWrapper<SR> {
//         ReaderWrapper {
//             inner: Some(store_reader),
//         }
//     }
// }

impl<S: Reader> ObjectSafeReader for ReaderWrapper<S> {
    fn object_safe_read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
        callback: &mut dyn FnMut(
            Option<Box<dyn erased_serde::Deserializer<'de>>>,
        ) -> Result<(), Error>,
    ) -> Result<(), Error>
    where
        'this: 'de,
    {
        callback(self.as_mut().read_to_deserializer(from)?)
    }
}

pub(crate) struct ReaderWrapper<S> {
    inner: Option<S>,
}

impl<S> ReaderWrapper<S> {
    pub(crate) fn as_mut(&mut self) -> &mut S {
        self.inner.as_mut().unwrap()
    }
}

pub trait Store: Writer + Reader {}
impl<T> Store for T where T: Writer + Reader {}

pub(crate) trait ObjectSafeStore: ObjectSafeWriter + ObjectSafeReader {}
impl<T> ObjectSafeStore for T where T: ObjectSafeWriter + ObjectSafeReader {}

// impl Reader for dyn ObjectSafeStore {
//     fn read_to_deserializer<'de, 'this>(
//         &'this mut self,
//         from: &Path,
//     ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
//     where
//         'this: 'de,
//     {
//         let mut maybe_deserializer: Option<Box<dyn erased_serde::Deserializer<'de>>> = None;
//         {
//             let mut callback = |maybe_erased: Option<Box<dyn erased_serde::Deserializer<'de>>>| {
//                 if let Some(erased) = maybe_erased {
//                     maybe_deserializer.insert(erased);
//                 }

//                 Ok(())
//             };
//             self.object_safe_read_to_deserializer(from, &mut callback)?;
//         }

//         Ok(maybe_deserializer)
//     }

//     fn read_owned<RecordType: DeserializeOwned>(
//         &mut self,
//         from: &Path,
//     ) -> Result<Option<RecordType>, Error> {
//         if let Some(deserializer) = self.read_to_deserializer(from)? {
//             let record = RecordType::deserialize(deserializer).map_err(|error| {
//                 Error::RecordDeserializationError {
//                     message: error.to_string(),
//                 }
//             })?;
//             return Ok(Some(record));
//         } else {
//             Ok(None)
//         }
//     }
// }

// impl Writer for dyn ObjectSafeStore {
//     fn write<RecordType: Serialize>(
//         &mut self,
//         destination: &Path,
//         data: RecordType,
//     ) -> Result<Path, Error> {
//         self.object_safe_write(destination, &data)
//     }
// }

impl dyn ObjectSafeStore {
    pub(crate) fn erase<S: Store>(store: S) -> StoreWrapper<S> {
        StoreWrapper { inner: Some(store) }
    }
}

pub(crate) struct StoreWrapper<S> {
    inner: Option<S>,
}

impl<S> StoreWrapper<S> {
    pub(crate) fn as_mut(&mut self) -> &mut S {
        self.inner.as_mut().unwrap()
    }
}

impl<S: Store> ObjectSafeReader for StoreWrapper<S> {
    fn object_safe_read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
        callback: &mut dyn FnMut(
            Option<Box<dyn erased_serde::Deserializer<'de>>>,
        ) -> Result<(), Error>,
    ) -> Result<(), Error>
    where
        'this: 'de,
    {
        callback(self.as_mut().read_to_deserializer(from)?)
    }
}

impl<S: Store> ObjectSafeWriter for StoreWrapper<S> {
    fn object_safe_write(
        &mut self,
        to: &Path,
        data: &dyn erased_serde::Serialize,
    ) -> Result<Path, Error> {
        self.as_mut().write(to, data)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum LocalStoreError {
    #[error("An error occurred trying to read a the root path {path}: {error}")]
    RootPathInvalid {
        path: std::path::PathBuf,
        error: io::Error,
    },
    #[error("{0}")]
    StoreError(#[from] Error),
    #[error("{0}")]
    SerializationError(#[from] serde_json::error::Error),
}

impl From<LocalStoreError> for Error {
    fn from(error: LocalStoreError) -> Self {
        match error {
            LocalStoreError::RootPathInvalid { path: _, error: _ } => {
                Error::ImplementationFailure {
                    message: format!("{}", error),
                }
            }
            LocalStoreError::SerializationError(error) => Error::ImplementationFailure {
                message: format!("{}", error),
            },
            LocalStoreError::StoreError(store_error) => store_error,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod trait_test_suite {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    pub struct SimpleStruct {
        pub example: String,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    pub struct MoreComplexStruct {
        pub sub_struct: SimpleStruct,
        pub array_of_things: Vec<SimpleStruct>,
    }

    pub fn read_owned_simple_struct_works(store: &mut impl Reader) {
        let root_path = Path::parse("").unwrap();
        let actual_struct_value: SimpleStruct = store.read_owned(&root_path).unwrap().unwrap();
        let expected_struct_value = SimpleStruct {
            example: "Hello, world!".to_string(),
        };
        assert_eq!(actual_struct_value, expected_struct_value);

        let field_path = Path::parse("example").unwrap();
        let actual_field_value: String = store
            .read_owned(&root_path.join(&field_path))
            .unwrap()
            .unwrap();
        let expected_field_value = expected_struct_value.example;
        assert_eq!(actual_field_value, expected_field_value);
    }

    pub fn read_owned_complex_struct_works(store: &mut impl Reader) {
        let root_path = Path::parse("").unwrap();
        let actual_value: MoreComplexStruct = store.read_owned(&root_path).unwrap().unwrap();
        let expected_value = MoreComplexStruct {
            sub_struct: SimpleStruct {
                example: "Hello, world!".to_string(),
            },
            array_of_things: vec![
                SimpleStruct {
                    example: "Oh my goodness.".to_string(),
                },
                SimpleStruct {
                    example: "Look how working this is".to_string(),
                },
                SimpleStruct {
                    example: "Just so functional!".to_string(),
                },
            ],
        };
        assert_eq!(actual_value, expected_value);
    }

    pub fn write_works<S: Store>(store_factory: fn() -> S) {
        {
            let mut store = store_factory();

            let root_path = Path::parse("").unwrap();
            let expected_value = SimpleStruct {
                example: "Hello, world!".to_string(),
            };

            store.write(&root_path, &expected_value).unwrap();

            let actual_value: SimpleStruct = store.read_owned(&root_path).unwrap().unwrap();
            assert_eq!(actual_value, expected_value);
        }

        {
            let mut store = store_factory();

            let root_path = Path::parse("").unwrap();
            let expected_value = MoreComplexStruct {
                sub_struct: SimpleStruct {
                    example: "Hello, world!".to_string(),
                },
                array_of_things: vec![
                    SimpleStruct {
                        example: "Oh my goodness.".to_string(),
                    },
                    SimpleStruct {
                        example: "Look how working this is".to_string(),
                    },
                    SimpleStruct {
                        example: "Just so functional!".to_string(),
                    },
                ],
            };
            store.write(&root_path, &expected_value).unwrap();

            let actual_value: MoreComplexStruct = store.read_owned(&root_path).unwrap().unwrap();
            assert_eq!(actual_value, expected_value);
        }
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct ArrayTestStructChildStruct {
        children: Vec<SimpleStruct>,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct ArrayStructception {
        more_children: Vec<ArrayTestStructChildStruct>,
    }

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct ArrayTestStruct {
        bool_array: Vec<bool>,
        int_array: Vec<i32>,
        float_array: Vec<f32>,
        string_array: Vec<String>,
        struct_array: Vec<SimpleStruct>,
        array_array: Vec<Vec<bool>>,
        array_struct_array: Vec<ArrayTestStructChildStruct>,
        struct_array_struct_array: Vec<ArrayStructception>,
    }

    pub fn arrays_work<S: Store>(store_factory: fn() -> S) {
        let mut store = store_factory();

        let root_path = Path::parse("").unwrap();
        let test_value = ArrayTestStruct {
            bool_array: vec![true, false, false, true],
            int_array: vec![5, 4, 3, 2, 1, 0, -1, -2, -3, -4, -5],
            float_array: vec![std::f32::consts::PI, 1.618033, 0.577215],
            string_array: vec![
                "zero".to_string(),
                "one".to_string(),
                "two".to_string(),
                "three".to_string(),
                "four".to_string(),
            ],
            struct_array: vec![
                SimpleStruct {
                    example: "Oh my goodness.".to_string(),
                },
                SimpleStruct {
                    example: "Look how working this is".to_string(),
                },
                SimpleStruct {
                    example: "Just so functional!".to_string(),
                },
            ],
            array_array: vec![
                vec![true, false, false],
                vec![false, true, false],
                vec![false, false, true],
            ],
            array_struct_array: vec![
                ArrayTestStructChildStruct {
                    children: vec![
                        SimpleStruct {
                            example: "Zero".to_string(),
                        },
                        SimpleStruct {
                            example: "One".to_string(),
                        },
                        SimpleStruct {
                            example: "Two".to_string(),
                        },
                    ],
                },
                ArrayTestStructChildStruct {
                    children: vec![
                        SimpleStruct {
                            example: "Three".to_string(),
                        },
                        SimpleStruct {
                            example: "Four".to_string(),
                        },
                        SimpleStruct {
                            example: "Five".to_string(),
                        },
                    ],
                },
                ArrayTestStructChildStruct {
                    children: vec![
                        SimpleStruct {
                            example: "Six".to_string(),
                        },
                        SimpleStruct {
                            example: "Seven".to_string(),
                        },
                        SimpleStruct {
                            example: "Eight".to_string(),
                        },
                    ],
                },
            ],
            struct_array_struct_array: vec![
                ArrayStructception {
                    more_children: vec![
                        ArrayTestStructChildStruct {
                            children: vec![
                                SimpleStruct {
                                    example: "Zero Zero".to_string(),
                                },
                                SimpleStruct {
                                    example: "Zero One".to_string(),
                                },
                                SimpleStruct {
                                    example: "Zero Two".to_string(),
                                },
                            ],
                        },
                        ArrayTestStructChildStruct {
                            children: vec![
                                SimpleStruct {
                                    example: "Zero Three".to_string(),
                                },
                                SimpleStruct {
                                    example: "Zero Four".to_string(),
                                },
                                SimpleStruct {
                                    example: "Zero Five".to_string(),
                                },
                            ],
                        },
                    ],
                },
                ArrayStructception {
                    more_children: vec![
                        ArrayTestStructChildStruct {
                            children: vec![
                                SimpleStruct {
                                    example: "One Zero".to_string(),
                                },
                                SimpleStruct {
                                    example: "One One".to_string(),
                                },
                                SimpleStruct {
                                    example: "One Two".to_string(),
                                },
                            ],
                        },
                        ArrayTestStructChildStruct {
                            children: vec![
                                SimpleStruct {
                                    example: "One Three".to_string(),
                                },
                                SimpleStruct {
                                    example: "One Four".to_string(),
                                },
                                SimpleStruct {
                                    example: "One Five".to_string(),
                                },
                            ],
                        },
                    ],
                },
            ],
        };
        store.write(&root_path, &test_value).unwrap();

        let actual_value: ArrayTestStruct = store.read_owned(&root_path).unwrap().unwrap();
        assert_eq!(actual_value, test_value);

        // Test simple types
        for (i, v) in test_value.bool_array.iter().enumerate() {
            let path = Path::parse(&format!("bool_array/{}", i)).unwrap();
            assert_eq!(&store.read_owned::<bool>(&path).unwrap().unwrap(), v)
        }
        for (i, v) in test_value.int_array.iter().enumerate() {
            let path = Path::parse(&format!("int_array/{}", i)).unwrap();
            assert_eq!(&store.read_owned::<i32>(&path).unwrap().unwrap(), v)
        }
        for (i, v) in test_value.float_array.iter().enumerate() {
            let path = Path::parse(&format!("float_array/{}", i)).unwrap();
            assert_eq!(&store.read_owned::<f32>(&path).unwrap().unwrap(), v)
        }
        for (i, v) in test_value.string_array.iter().enumerate() {
            let path = Path::parse(&format!("string_array/{}", i)).unwrap();
            assert_eq!(&store.read_owned::<String>(&path).unwrap().unwrap(), v)
        }

        // Test composites
        let example_path = Path::parse("example").unwrap();
        for (i, v) in test_value.struct_array.iter().enumerate() {
            let path = Path::parse(&format!("struct_array/{}", i)).unwrap();
            assert_eq!(
                &store.read_owned::<SimpleStruct>(&path).unwrap().unwrap(),
                v
            );

            let field_path = path.join(&example_path);
            assert_eq!(
                store.read_owned::<String>(&field_path).unwrap().unwrap(),
                v.example
            );
        }

        for (i, v) in test_value.array_array.iter().enumerate() {
            let path = Path::parse(&format!("array_array/{}", i)).unwrap();
            assert_eq!(&store.read_owned::<Vec<bool>>(&path).unwrap().unwrap(), v)
        }

        let children_path = Path::parse("children").unwrap();
        for (i, outer_child) in test_value.array_struct_array.iter().enumerate() {
            let path = Path::parse(&format!("array_struct_array/{}", i)).unwrap();
            assert_eq!(
                &store
                    .read_owned::<ArrayTestStructChildStruct>(&path)
                    .unwrap()
                    .unwrap(),
                outer_child
            );

            let sub_struct_path = path.join(&children_path);
            for (i, leaf) in outer_child.children.iter().enumerate() {
                let leaf_path = sub_struct_path.join(&Path::parse(&format!("{}", i)).unwrap());
                assert_eq!(
                    &store
                        .read_owned::<SimpleStruct>(&leaf_path)
                        .unwrap()
                        .unwrap(),
                    leaf
                );

                let field_path = leaf_path.join(&example_path);
                assert_eq!(
                    store.read_owned::<String>(&field_path).unwrap().unwrap(),
                    leaf.example
                );
            }
        }

        for (i, v) in test_value.struct_array_struct_array.iter().enumerate() {
            let path = Path::parse(&format!("struct_array_struct_array/{}", i)).unwrap();
            assert_eq!(
                &store
                    .read_owned::<ArrayStructception>(&path)
                    .unwrap()
                    .unwrap(),
                v
            )
            // TODO(akesling): Implement lookup tests for children here.
        }
    }
}
