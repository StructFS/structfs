use serde_json::value::Value as JsonValue;
use structfs_store::{Error as StoreError, Path, PathError};

pub fn get_sub_tree_mut<'tree>(
    tree: &'tree mut JsonValue,
    path: &Path,
) -> Result<Option<&'tree mut JsonValue>, StoreError> {
    let mut cursor: &mut JsonValue = tree;
    for component in path.iter() {
        match cursor {
            JsonValue::Object(map) => {
                if !map.contains_key(component) {
                    return Ok(None);
                }
                cursor = map.get_mut(component).ok_or_else(|| {
                    StoreError::from(PathError::PathInvalid {
                        path: Path {
                            components: path.iter().map(|s| s.to_owned()).collect(),
                        },
                        message: format!(
                            "Path not found in store.  Lookup failed at component ({}).",
                            component
                        ),
                    })
                })?;
                continue;
            }
            JsonValue::Array(arr) => {
                let index = component.parse::<usize>().map_err(|error| {
                    StoreError::from(PathError::PathInvalid {
                        path: Path {
                            components: path.iter().map(|s| s.to_owned()).collect(),
                        },
                        message: format!(
                            concat!(
                                "Path not found in store as a path component ({}) ",
                                "failed to parse as an array index: {}."
                            ),
                            component, error
                        ),
                    })
                })?;

                if let Some(entry) = arr.get_mut(index) {
                    cursor = entry;
                } else {
                    return Ok(None);
                }
            }
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
                // TODO(akesling): Improve the quality of these errors by saying _where_ in the
                // path the lookup failure occurred.  Right now this just says "oops your path
                // was bad... FIGURE IT OUT FOR YOURSELF!1!!" and this just isn't friendly.
                // Don't we want to be friendly? DON'T WE?
                return Err(StoreError::from(PathError::PathInvalid {
                    path: Path {
                        components: path.iter().map(|s| s.to_owned()).collect(),
                    },
                    message: format!(
                        "Path not found in store.  Lookup failed at component ({}).",
                        component
                    ),
                }));
            }
        }
    }

    Ok(Some(cursor))
}

pub fn get_sub_tree<'tree>(
    tree: &'tree JsonValue,
    path: &Path,
) -> Result<Option<&'tree JsonValue>, StoreError> {
    let mut cursor: &JsonValue = tree;
    for component in path.iter() {
        match cursor {
            JsonValue::Object(map) => {
                if !map.contains_key(component) {
                    return Ok(None);
                }
                cursor = map.get(component).ok_or_else(|| {
                    StoreError::from(PathError::PathInvalid {
                        path: Path {
                            components: path.iter().map(|s| s.to_owned()).collect(),
                        },
                        message: format!(
                            "Path not found in store.  Lookup failed at component ({}).",
                            component
                        ),
                    })
                })?;
                continue;
            }
            JsonValue::Array(arr) => {
                let index = component.parse::<usize>().map_err(|error| {
                    StoreError::from(PathError::PathInvalid {
                        path: Path {
                            components: path.iter().map(|s| s.to_owned()).collect(),
                        },
                        message: format!(
                            concat!(
                                "Path not found in store as a path component ({}) ",
                                "failed to parse as an array index: {}."
                            ),
                            component, error
                        ),
                    })
                })?;

                if let Some(entry) = arr.get(index) {
                    cursor = entry;
                } else {
                    return Ok(None);
                }
            }
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
                // TODO(akesling): Improve the quality of these errors by saying _where_ in the
                // path the lookup failure occurred.  Right now this just says "oops your path
                // was bad... FIGURE IT OUT FOR YOURSELF!1!!" and this just isn't friendly.
                // Don't we want to be friendly? DON'T WE?
                return Err(StoreError::from(PathError::PathInvalid {
                    path: Path {
                        components: path.iter().map(|s| s.to_owned()).collect(),
                    },
                    message: format!(
                        "Path not found in store.  Lookup failed at component ({}).",
                        component
                    ),
                }));
            }
        }
    }

    Ok(Some(cursor))
}

pub fn set_path(tree: &mut JsonValue, path: &Path, value: JsonValue) -> Result<(), StoreError> {
    let path_len = path.components.len();
    let (sub_tree, last_path_component) = if path_len == 0 {
        *tree = value;
        return Ok(());
    } else if path_len == 1 {
        (tree, &path[0])
    } else {
        let path_prefix = &path.slice_as_path(0, path_len - 1);
        let last_path_component = &path[path_len - 1];
        let sub_tree = get_sub_tree_mut(tree, path_prefix)?.ok_or_else(|| {
            StoreError::from(PathError::PathInvalid {
                path: path.slice_as_path(0, path_prefix.len()),
                message: "Path prefix does not exist when setting path.".to_string(),
            })
        })?;
        (sub_tree, last_path_component)
    };

    match sub_tree {
        JsonValue::Object(map) => {
            map.insert(last_path_component.clone(), value);
        }
        JsonValue::Array(arr) => {
            let index = last_path_component.parse::<usize>().map_err(|error| {
                StoreError::from(PathError::PathInvalid {
                    path: path.clone(),
                    message: format!(
                        concat!(
                            "Path not found in store as final path component ({}) ",
                            "failed to parse as an array index: {}."
                        ),
                        last_path_component, error
                    ),
                })
            })?;

            // This comparison is more readable than index.cmp(arr.len) matching against
            // Ordering::Less, etc.
            #[allow(clippy::comparison_chain)]
            if index < arr.len() {
                arr[index] = value;
            } else if index == arr.len() {
                arr.push(value);
            } else {
                return Err(StoreError::from(PathError::PathInvalid {
                    path: path.clone(),
                    message: format!(
                        concat!(
                            "Path not found in store as the final path component ({}) ",
                            "was not in bounds for the indexed array.",
                        ),
                        last_path_component
                    ),
                }));
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
            return Err(StoreError::from(PathError::PathInvalid {
                path: path.clone(),
                message: "Path not found in store.".to_string(),
            }))
        }
    }

    Ok(())
}

pub fn get_path<'tree>(
    tree: &'tree JsonValue,
    path: &Path,
) -> Result<Option<&'tree JsonValue>, StoreError> {
    let path_len = path.components.len();
    let (sub_tree, last_path_component) = if path_len == 0 {
        return Ok(Some(tree));
    } else if path_len == 1 {
        (tree, &path[0])
    } else {
        let path_prefix = &path.slice_as_path(0, path_len - 1);
        let last_path_component = &path[path_len - 1];
        let sub_tree = match get_sub_tree(tree, path_prefix)? {
            Some(tree) => tree,
            None => {
                return Ok(None);
            }
        };
        (sub_tree, last_path_component)
    };
    let sub_tree: &JsonValue = sub_tree;

    match sub_tree {
        JsonValue::Object(map) => Ok(map.get(last_path_component.as_str())),
        JsonValue::Array(arr) => {
            let index = last_path_component.parse::<usize>().map_err(|error| {
                StoreError::from(PathError::PathInvalid {
                    path: path.clone(),
                    message: format!(
                        concat!(
                            "Path not found in store as final path component ({}) ",
                            "failed to parse as an array index: {}."
                        ),
                        last_path_component, error
                    ),
                })
            })?;

            if index < arr.len() {
                Ok(Some(&arr[index]))
            } else {
                Err(StoreError::from(PathError::PathInvalid {
                    path: path.clone(),
                    message: format!(
                        concat!(
                            "Path not found in store as the final path component ({}) ",
                            "was not in bounds for the indexed array.",
                        ),
                        last_path_component
                    ),
                }))
            }
        }
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod get_path_tests {
    use super::*;
    use serde_json::json;
    use structfs_store::path;

    #[test]
    fn empty() {
        assert_eq!(
            &get_path(&json!({}), &path!("")).unwrap(),
            &Some(&json!({}))
        );
    }

    #[test]
    fn look_up_root() {
        let tree = json!({
            "hello": "utils!",
        });
        assert_eq!(&get_path(&tree, &path!("")).unwrap(), &Some(&tree));
    }

    #[test]
    fn look_up_atom_path() {
        let tree = json!({
            "one": 1,
            "nest_1": {
                "three": 3,
                "nest_2": {
                    "four": 4,
                    "five": 5,
                }
            }
        });
        assert_eq!(
            &get_path(&tree, &path!("nest_1/nest_2/four")).unwrap(),
            &Some(&json!(4))
        );
    }
}
