use std::{ffi, fs, io, path};

use lazy_static::lazy_static;
use regex::Regex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use serde_json::value::Value as JsonValue;

use crate::json_utils;
use structfs_store::{
    Error as StoreError, LocalStoreError, Path, Reader as StoreRead, Writer as StoreWrite,
};

// TODO(alex): Decouple LocalStore from (de)serializer so JSON, Flexbuffers, etc. can be
// parameterized for any LocalStore.
pub struct JSONLocalStore {
    root: path::PathBuf,
}

impl JSONLocalStore {
    pub fn new(root: path::PathBuf) -> Result<JSONLocalStore, LocalStoreError> {
        let attr = fs::metadata(&root).map_err(|error| LocalStoreError::RootPathInvalid {
            path: root.clone(),
            error,
        })?;

        if !attr.is_dir() {
            return Err(LocalStoreError::RootPathInvalid {
                path: root,
                error: io::Error::other("Root path must be a directory."),
            });
        }

        // TODO(alex): Add config parameters that allow for readonly stores.
        if attr.permissions().readonly() {
            return Err(LocalStoreError::RootPathInvalid {
                path: root,
                error: io::Error::other("Root directory must be writable"),
            });
        }

        match root.canonicalize() {
            Ok(root) => Ok(JSONLocalStore { root }),
            Err(error) => Err(LocalStoreError::RootPathInvalid { path: root, error }),
        }
    }

    fn store_path_to_file_path(&self, path: &Path) -> Result<path::PathBuf, LocalStoreError> {
        Ok(self
            .root
            .components()
            .chain(
                path.components
                    .iter()
                    .map(|s| path::Component::Normal(ffi::OsStr::new(s))),
            )
            .collect())
    }

    fn overwrite_path_with_directory(
        file_path: &path::Path,
        path: &Path,
    ) -> Result<(), LocalStoreError> {
        if file_path.exists() {
            let attr = fs::metadata(file_path).map_err(|err| {
                LocalStoreError::from(StoreError::ImplementationFailure {
                    message: format!(
                        concat!(
                            "File path ({}) generated from store path ({:?}) could not be ",
                            "accessed with error when getting file metadata: {:?}"
                        ),
                        file_path.display(),
                        path,
                        err
                    ),
                })
            })?;

            if attr.is_file() {
                fs::remove_file(file_path).map_err(|err| {
                    LocalStoreError::from(StoreError::ImplementationFailure {
                        message: format!(
                            concat!(
                                "File path ({}) generated from store path ({:?}) could not be ",
                                "removed with error when overwriting with object path ",
                                "directory: {:?}"
                            ),
                            file_path.display(),
                            path,
                            err
                        ),
                    })
                })?;
            }
        }

        fs::create_dir_all(file_path).map_err(|err| {
            LocalStoreError::from(StoreError::ImplementationFailure {
                message: format!(
                    concat!(
                        "File path ({}) generated from store path ({:?}) could not be accessed ",
                        "with error when creating directory: {:?}"
                    ),
                    file_path.display(),
                    path,
                    err
                ),
            })
        })?;

        Ok(())
    }

    fn write_value(&self, to: &Path, value: &JsonValue) -> Result<(), LocalStoreError> {
        use io::Write;

        let file_path = self.store_path_to_file_path(to)?;

        match value {
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
                log::debug!("Writing {}...", file_path.display());

                let s = serde_json::to_string(value)?;

                let mut f = fs::File::create(&file_path).map_err(|err| {
                    LocalStoreError::from(StoreError::ImplementationFailure {
                        message: format!(
                            concat!(
                                "File path ({}) generated from store path ({:?}) could not be ",
                                "accessed with error when writing leaf: {:?}"
                            ),
                            &file_path.display(),
                            to,
                            err
                        ),
                    })
                })?;

                f.write_all(s.as_bytes()).unwrap();
            }
            JsonValue::Array(arr) => {
                Self::overwrite_path_with_directory(&file_path, to)?;
                for (i, val) in arr.iter().enumerate() {
                    self.write_value(
                        &to.join(&Path {
                            components: vec![format!("{}", i)],
                        }),
                        val,
                    )?;
                }
            }
            JsonValue::Object(map) => {
                Self::overwrite_path_with_directory(&file_path, to)?;
                for (map_path, val) in map.iter() {
                    self.write_value(
                        &to.join(&Path {
                            components: vec![map_path.to_owned()],
                        }),
                        val,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn read_value(&self, from: &Path) -> Result<Option<JsonValue>, LocalStoreError> {
        lazy_static! {
            static ref NUMERIC_VALUED_PATH: Regex =
                Regex::new(r"^/?([^/]+/)*([1-9][0-9]*|0)$").unwrap();
        }

        let file_path: path::PathBuf = self.store_path_to_file_path(from)?;
        log::debug!("Reading {}...", file_path.display());
        if !file_path.exists() {
            return Ok(None);
        }

        let store_root = self.root.as_path();
        let mut value_tree = JsonValue::Null;
        for entry in walkdir::WalkDir::new(file_path)
            // Make sure arrays are visited in index order.
            .sort_by(|a, b| {
                let a_as_index = a.file_name().to_str().map(|v| v.parse::<usize>());
                let b_as_index = b.file_name().to_str().map(|v| v.parse::<usize>());
                if let (Some(Ok(a_index)), Some(Ok(b_index))) = (a_as_index, b_as_index) {
                    a_index.cmp(&b_index)
                } else {
                    a.file_name().cmp(b.file_name())
                }
            })
            .into_iter()
            .filter_map(Result::ok)
        {
            let absolute_path = entry.path();
            let relative_path = absolute_path.strip_prefix(store_root).map_err(|e| {
                StoreError::ImplementationFailure {
                    message: format!(
                        "Failed to strip store root prefix from dir entry path: {:?}",
                        e
                    ),
                }
            })?;

            let relative_path = Path::parse(relative_path.to_str().unwrap())
                .map_err(StoreError::from)?
                .strip_prefix(from)
                .unwrap();
            if entry.file_type().is_dir() {
                let mut dir_paths = fs::read_dir(entry.path()).map_err(|error| {
                    StoreError::ImplementationFailure {
                        message: format!(
                            "Failed to read directory ({}): {:?}",
                            entry.path().display(),
                            error
                        ),
                    }
                })?;
                let all_numeric_paths = dir_paths.all(|p| {
                    p.is_ok_and(|entry| {
                        let path = entry.path();
                        path.to_str()
                            .is_some_and(|s| NUMERIC_VALUED_PATH.is_match(s))
                    })
                });

                json_utils::set_path(
                    &mut value_tree,
                    &relative_path,
                    if all_numeric_paths {
                        json!([])
                    } else {
                        json!({})
                    },
                )?;
            } else {
                let file = fs::File::open(absolute_path).map_err(|e| {
                    LocalStoreError::from(StoreError::ImplementationFailure {
                        message: format!(
                            concat!(
                                "Error occurred when reading from path '{}' ",
                                "while accessing path {:?}: {:?}"
                            ),
                            absolute_path.display(),
                            relative_path,
                            e
                        ),
                    })
                })?;
                let reader = io::BufReader::new(file);
                let value: JsonValue = serde_json::from_reader(reader)?;
                json_utils::set_path(&mut value_tree, &relative_path, value)?;
            }
        }

        Ok(Some(value_tree))
    }

    // TODO(alex): Add procedure for debugging/validating the integrity of a local store.
}

#[cfg(test)]
mod json_local_store_tests {
    use super::*;

    #[test]
    fn works() {
        let dir = tempfile::tempdir().unwrap();
        let store = JSONLocalStore::new(path::PathBuf::from(dir.path())).unwrap();

        // Add the root structure.
        store
            .write_value(
                &Path::parse("").unwrap(),
                &json!({
                    "example": "Hello, world!",
                }),
            )
            .unwrap();
        // Add a new path pointing to a structure.
        store
            .write_value(
                &Path::parse("foo").unwrap(),
                &json!({
                    "bar": "baz",
                }),
            )
            .unwrap();
        // Overwrite an existing path with a new structure.
        store
            .write_value(&Path::parse("foo/bar").unwrap(), &json!({}))
            .unwrap();
        store
            .write_value(
                &Path::parse("foo/bar/baz").unwrap(),
                &json!({
                    "quuux": 1,
                    "flub": 1.2,
                }),
            )
            .unwrap();

        let root_path = Path::parse("").unwrap();
        let actual_value = store.read_value(&root_path).unwrap().unwrap();
        let expected_value = json!({
            "example": "Hello, world!",
            "foo": {
                "bar": {
                    "baz": {
                        "quuux": 1,
                        "flub": 1.2,
                    },
                },
            },
        });
        assert_eq!(actual_value, expected_value);
    }

    #[test]
    fn read_value_works() {
        use io::Write;

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("example");

        let mut f = fs::File::create(file_path).unwrap();
        f.write_all(b"\"Hello, world!\"").unwrap();
        f.sync_all().unwrap();

        let store = JSONLocalStore::new(path::PathBuf::from(dir.path())).unwrap();

        let root_path = Path::parse("").unwrap();
        let actual_value = store.read_value(&root_path).unwrap().unwrap();
        let expected_value = json!({
            "example": "Hello, world!",
        });
        assert_eq!(actual_value, expected_value);
    }

    #[test]
    fn write_value_works() {
        let json_value = json!({
            "Hello": "World!",
            "and": "other things",
        });

        let dir = tempfile::tempdir().unwrap();
        let store = JSONLocalStore::new(path::PathBuf::from(dir.path())).unwrap();

        store
            .write_value(&Path::parse("foo/bar/baz").unwrap(), &json_value)
            .unwrap();

        let root_path = Path::parse("").unwrap();
        let actual_value = store.read_value(&root_path).unwrap().unwrap();
        let expected_value = json!({
            "foo": {
                "bar": {
                    "baz": json_value,
                },
            },
        });
        assert_eq!(actual_value, expected_value);
    }
}

impl StoreWrite for JSONLocalStore {
    fn write<D: Serialize>(&mut self, destination: &Path, data: D) -> Result<Path, StoreError> {
        let value = serde_json::to_value(data).map_err(LocalStoreError::from)?;
        self.write_value(destination, &value)?;

        Ok(destination.clone())
    }
}

impl StoreRead for JSONLocalStore {
    // type Deserializer<'de> = serde_json::Value;

    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        Ok(self.read_value(from)?.map(|v| {
            let de: Box<dyn erased_serde::Deserializer> =
                Box::new(<dyn erased_serde::Deserializer>::erase(v));
            de
        }))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        if let Some(value) = self.read_value(from)? {
            // TODO(akesling): Better communicate error on type mismatch between found record and
            // requested type.
            let data: RecordType = serde_json::from_value(value)
                .map_err(LocalStoreError::from)
                .map_err(|err| StoreError::RecordSerialization {
                    message: format!("{}", err),
                })?;

            Ok(Some(data))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod store_impl_for_json_local_store_tests {
    use super::*;

    use structfs_store::store::trait_test_suite;
    use structfs_store::store::trait_test_suite::SimpleStruct;

    #[test]
    fn read_owned_works() {
        use io::Write;

        {
            let dir = tempfile::tempdir().unwrap();
            let file_path = dir.path().join("example");

            let mut f = fs::File::create(file_path).unwrap();
            f.write_all(b"\"Hello, world!\"").unwrap();
            f.sync_all().unwrap();

            let mut store = JSONLocalStore::new(path::PathBuf::from(dir.path())).unwrap();

            trait_test_suite::read_owned_simple_struct_works(&mut store);
        }

        {
            let dir = tempfile::tempdir().unwrap();
            let sub_struct_path = dir.path().join("sub_struct");
            fs::create_dir(&sub_struct_path).unwrap();
            let file_path = sub_struct_path.join("example");

            {
                let mut f = fs::File::create(file_path).unwrap();
                f.write_all(b"\"Hello, world!\"").unwrap();
                f.sync_all().unwrap();
            }

            let array_of_things_path = dir.path().join("array_of_things");
            fs::create_dir(&array_of_things_path).unwrap();
            let structs = [
                SimpleStruct {
                    example: "Oh my goodness.".to_string(),
                },
                SimpleStruct {
                    example: "Look how working this is".to_string(),
                },
                SimpleStruct {
                    example: "Just so functional!".to_string(),
                },
            ];

            for (i, s) in structs.iter().enumerate() {
                let struct_entry_dir_path = array_of_things_path.join(format!("{}", i));
                fs::create_dir(&struct_entry_dir_path).unwrap();
                let struct_file_path = struct_entry_dir_path.join("example");
                let mut f = fs::File::create(struct_file_path).unwrap();
                f.write_all(format!("\"{}\"", s.example).as_bytes())
                    .unwrap();
                f.sync_all().unwrap();
            }

            let mut store = JSONLocalStore::new(path::PathBuf::from(dir.path())).unwrap();

            trait_test_suite::read_owned_complex_struct_works(&mut store);
        }
    }

    struct TestJSONLocalStore {
        // Having this as a member allows the directory to be cleaned up once the test store is
        // dropped.
        _dir: tempfile::TempDir,
        store: JSONLocalStore,
    }

    impl TestJSONLocalStore {
        fn new() -> TestJSONLocalStore {
            let dir = tempfile::tempdir().unwrap();
            let dir_path = path::PathBuf::from(dir.path());
            TestJSONLocalStore {
                _dir: dir,
                store: JSONLocalStore::new(dir_path).unwrap(),
            }
        }
    }

    impl StoreWrite for TestJSONLocalStore {
        fn write<D: Serialize>(&mut self, destination: &Path, data: D) -> Result<Path, StoreError> {
            self.store.write(destination, data)
        }
    }

    impl StoreRead for TestJSONLocalStore {
        // type Deserializer<'de> = serde_json::Value;

        fn read_to_deserializer<'de, 'this>(
            &'this mut self,
            from: &Path,
        ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
        where
            'this: 'de,
        {
            self.store.read_to_deserializer(from)
        }

        fn read_owned<RecordType: DeserializeOwned>(
            &mut self,
            path: &Path,
        ) -> Result<Option<RecordType>, StoreError> {
            self.store.read_owned(path)
        }
    }

    #[test]
    fn write_works() {
        trait_test_suite::write_works(TestJSONLocalStore::new);
    }

    #[test]
    fn arrays_work() {
        trait_test_suite::arrays_work(TestJSONLocalStore::new);
    }
}
