use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::value::Value as JsonValue;

use crate::json_utils;
use structfs_store as store;
use structfs_store::Path;

pub struct SerdeJSONInMemoryStore {
    root: JsonValue,
}

impl SerdeJSONInMemoryStore {
    pub fn new() -> Result<SerdeJSONInMemoryStore, store::Error> {
        Ok(SerdeJSONInMemoryStore {
            root: JsonValue::Null,
        })
    }

    fn write_value(&mut self, to: &Path, value: JsonValue) -> Result<(), store::Error> {
        json_utils::set_path(&mut self.root, to, value)
    }

    fn read_value(&mut self, from: &Path) -> Result<Option<&JsonValue>, store::Error> {
        json_utils::get_path(&self.root, from)
    }
}

#[cfg(test)]
mod serde_json_in_memory_store_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn works() {
        let mut store = SerdeJSONInMemoryStore::new().unwrap();

        // Add the root structure.
        store
            .write_value(
                &Path::parse("").unwrap(),
                json!({
                    "example": "Hello, world!",
                }),
            )
            .unwrap();
        // Add a new path pointing to a structure.
        store
            .write_value(
                &Path::parse("foo").unwrap(),
                json!({
                    "bar": "baz",
                    "flibbity": [1, 2, 3, 4, 5],
                }),
            )
            .unwrap();
        // Overwrite an existing path with a new structure.
        store
            .write_value(&Path::parse("foo/bar").unwrap(), json!({}))
            .unwrap();
        store
            .write_value(
                &Path::parse("foo/bar/baz").unwrap(),
                json!({
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
                "flibbity": [1, 2, 3, 4, 5],
            },
        });
        assert_eq!(actual_value, &expected_value);
    }
}

impl store::Writer for SerdeJSONInMemoryStore {
    fn write<D: Serialize>(&mut self, destination: &Path, data: D) -> Result<Path, store::Error> {
        let value = serde_json::to_value(data).map_err(store::LocalStoreError::from)?;
        self.write_value(destination, value)?;

        Ok(destination.clone())
    }
}

impl store::Reader for SerdeJSONInMemoryStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, store::Error>
    where
        'this: 'de,
    {
        Ok(self.read_value(from)?.map(|v| {
            let de: Box<dyn erased_serde::Deserializer> =
                Box::new(<dyn erased_serde::Deserializer>::erase(v.clone()));
            de
        }))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, store::Error> {
        if let Some(value) = self.read_value(from)? {
            // TODO(akesling): Handle type mismatch error between dynamic JSON value in store and
            // caller-requested static type.
            let data: RecordType = serde_json::from_value(value.clone())
                .map_err(store::LocalStoreError::from)
                .map_err(|err| store::Error::RecordDeserialization {
                    message: format!(
                        concat!(
                            "Value ({:?}) at path '{}' could not be transformed ",
                            "to Rust type with error: {}"
                        ),
                        value, from, err
                    ),
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
    use serde_json::json;

    #[test]
    fn read_owned_works() {
        {
            let mut store = SerdeJSONInMemoryStore::new().unwrap();
            store
                .write_value(
                    &Path::parse("").unwrap(),
                    json!({
                        "example": "Hello, world!",
                    }),
                )
                .unwrap();

            trait_test_suite::read_owned_simple_struct_works(&mut store);
        }

        {
            let mut store = SerdeJSONInMemoryStore::new().unwrap();
            store
                .write_value(
                    &Path::parse("").unwrap(),
                    json!({
                        "sub_struct": {
                            "example": "Hello, world!",
                        },
                        "array_of_things": [
                            {
                                "example": "Oh my goodness.",
                            },
                            {
                                "example": "Look how working this is",
                            },
                            {
                                "example": "Just so functional!",
                            },
                        ],
                    }),
                )
                .unwrap();

            trait_test_suite::read_owned_complex_struct_works(&mut store);
        }
    }

    #[test]
    fn write_works() {
        trait_test_suite::write_works(|| SerdeJSONInMemoryStore::new().unwrap())
    }

    #[test]
    fn arrays_work() {
        trait_test_suite::arrays_work(|| SerdeJSONInMemoryStore::new().unwrap())
    }
}
