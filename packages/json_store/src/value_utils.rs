//! Utilities for working with core_store::Value trees.
//!
//! Utility functions for navigating and modifying Value trees.

use std::collections::BTreeMap;

use structfs_core_store::{Error, Path, PathError, Value};

/// Get a reference to a sub-tree at the given path.
pub fn get_path<'a>(tree: &'a Value, path: &Path) -> Result<Option<&'a Value>, Error> {
    if path.is_empty() {
        return Ok(Some(tree));
    }

    let mut cursor = tree;
    for (i, component) in path.iter().enumerate() {
        match cursor {
            Value::Map(map) => {
                if let Some(next) = map.get(component.as_str()) {
                    cursor = next;
                } else {
                    return Ok(None);
                }
            }
            Value::Array(arr) => {
                let index = component.parse::<usize>().map_err(|e| {
                    Error::Path(PathError::InvalidComponent {
                        component: component.clone(),
                        position: i,
                        message: format!("Expected array index, got: {}", e),
                    })
                })?;
                if let Some(next) = arr.get(index) {
                    cursor = next;
                } else {
                    return Ok(None);
                }
            }
            _ => {
                // Can't traverse into primitive values
                return Ok(None);
            }
        }
    }

    Ok(Some(cursor))
}

/// Get a mutable reference to a sub-tree at the given path.
pub fn get_path_mut<'a>(tree: &'a mut Value, path: &Path) -> Result<Option<&'a mut Value>, Error> {
    if path.is_empty() {
        return Ok(Some(tree));
    }

    let mut cursor = tree;
    for (i, component) in path.iter().enumerate() {
        match cursor {
            Value::Map(map) => {
                if !map.contains_key(component.as_str()) {
                    return Ok(None);
                }
                cursor = map.get_mut(component.as_str()).unwrap();
            }
            Value::Array(arr) => {
                let index = component.parse::<usize>().map_err(|e| {
                    Error::Path(PathError::InvalidComponent {
                        component: component.clone(),
                        position: i,
                        message: format!("Expected array index, got: {}", e),
                    })
                })?;
                if index >= arr.len() {
                    return Ok(None);
                }
                cursor = &mut arr[index];
            }
            _ => {
                return Ok(None);
            }
        }
    }

    Ok(Some(cursor))
}

/// Set a value at the given path.
///
/// Creates intermediate Map nodes if needed for single-component paths.
pub fn set_path(tree: &mut Value, path: &Path, value: Value) -> Result<(), Error> {
    if path.is_empty() {
        *tree = value;
        return Ok(());
    }

    let path_len = path.len();

    if path_len == 1 {
        // Set directly on tree
        set_child(tree, &path[0], value, 0)?;
        return Ok(());
    }

    // Navigate to parent
    let parent_path = path.slice(0, path_len - 1);
    let last_component = &path[path_len - 1];

    let parent = get_path_mut(tree, &parent_path)?.ok_or_else(|| {
        Error::Path(PathError::InvalidPath {
            message: format!("Parent path '{}' does not exist", parent_path),
        })
    })?;

    set_child(parent, last_component, value, path_len - 1)?;
    Ok(())
}

/// Set a child value on a Map or Array.
fn set_child(parent: &mut Value, key: &str, value: Value, position: usize) -> Result<(), Error> {
    match parent {
        Value::Map(map) => {
            map.insert(key.to_string(), value);
            Ok(())
        }
        Value::Array(arr) => {
            let index = key.parse::<usize>().map_err(|e| {
                Error::Path(PathError::InvalidComponent {
                    component: key.to_string(),
                    position,
                    message: format!("Expected array index, got: {}", e),
                })
            })?;

            if index < arr.len() {
                arr[index] = value;
            } else if index == arr.len() {
                arr.push(value);
            } else {
                return Err(Error::Path(PathError::InvalidComponent {
                    component: key.to_string(),
                    position,
                    message: format!("Array index {} out of bounds (len={})", index, arr.len()),
                }));
            }
            Ok(())
        }
        Value::Null => {
            // Auto-create a map for null values
            let mut map = BTreeMap::new();
            map.insert(key.to_string(), value);
            *parent = Value::Map(map);
            Ok(())
        }
        _ => Err(Error::Path(PathError::InvalidPath {
            message: format!("Cannot set child '{}' on primitive value", key),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use structfs_core_store::path;

    fn test_tree() -> Value {
        let mut root = BTreeMap::new();
        root.insert("name".to_string(), Value::String("Alice".to_string()));
        root.insert("age".to_string(), Value::Integer(30));

        let mut nested = BTreeMap::new();
        nested.insert("city".to_string(), Value::String("NYC".to_string()));
        root.insert("address".to_string(), Value::Map(nested));

        root.insert(
            "scores".to_string(),
            Value::Array(vec![
                Value::Integer(90),
                Value::Integer(85),
                Value::Integer(95),
            ]),
        );

        Value::Map(root)
    }

    // ==================== get_path tests ====================

    #[test]
    fn get_root() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn get_direct_child() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("name")).unwrap().unwrap();
        assert_eq!(result, &Value::String("Alice".to_string()));
    }

    #[test]
    fn get_nested_child() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("address/city")).unwrap().unwrap();
        assert_eq!(result, &Value::String("NYC".to_string()));
    }

    #[test]
    fn get_array_element() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("scores/1")).unwrap().unwrap();
        assert_eq!(result, &Value::Integer(85));
    }

    #[test]
    fn get_missing_returns_none() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_array_invalid_index_error() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("scores/abc"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected array index"));
    }

    #[test]
    fn get_array_out_of_bounds_none() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("scores/999")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_traverse_into_primitive_none() {
        let tree = test_tree();
        // Try to traverse into a string value
        let result = get_path(&tree, &path!("name/invalid")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_traverse_into_integer_none() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("age/invalid")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_nested_missing_intermediate() {
        let tree = test_tree();
        let result = get_path(&tree, &path!("address/missing/deep")).unwrap();
        assert!(result.is_none());
    }

    // ==================== get_path_mut tests ====================

    #[test]
    fn get_path_mut_root() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("")).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn get_path_mut_direct_child() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("name")).unwrap().unwrap();
        *result = Value::String("Bob".to_string());

        let check = get_path(&tree, &path!("name")).unwrap().unwrap();
        assert_eq!(check, &Value::String("Bob".to_string()));
    }

    #[test]
    fn get_path_mut_nested() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("address/city"))
            .unwrap()
            .unwrap();
        *result = Value::String("LA".to_string());

        let check = get_path(&tree, &path!("address/city")).unwrap().unwrap();
        assert_eq!(check, &Value::String("LA".to_string()));
    }

    #[test]
    fn get_path_mut_array_element() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("scores/0"))
            .unwrap()
            .unwrap();
        *result = Value::Integer(100);

        let check = get_path(&tree, &path!("scores/0")).unwrap().unwrap();
        assert_eq!(check, &Value::Integer(100));
    }

    #[test]
    fn get_path_mut_missing_returns_none() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_path_mut_array_invalid_index_error() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("scores/abc"));
        assert!(result.is_err());
    }

    #[test]
    fn get_path_mut_array_out_of_bounds_none() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("scores/999")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_path_mut_traverse_into_primitive_none() {
        let mut tree = test_tree();
        let result = get_path_mut(&mut tree, &path!("name/invalid")).unwrap();
        assert!(result.is_none());
    }

    // ==================== set_path tests ====================

    #[test]
    fn set_root_replaces_entire_tree() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!(""), Value::String("replaced".to_string())).unwrap();
        assert_eq!(tree, Value::String("replaced".to_string()));
    }

    #[test]
    fn set_on_null() {
        let mut tree = Value::Null;
        set_path(&mut tree, &path!("foo"), Value::String("bar".to_string())).unwrap();

        let result = get_path(&tree, &path!("foo")).unwrap().unwrap();
        assert_eq!(result, &Value::String("bar".to_string()));
    }

    #[test]
    fn set_overwrites() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!("name"), Value::String("Bob".to_string())).unwrap();

        let result = get_path(&tree, &path!("name")).unwrap().unwrap();
        assert_eq!(result, &Value::String("Bob".to_string()));
    }

    #[test]
    fn set_nested() {
        let mut tree = test_tree();
        set_path(
            &mut tree,
            &path!("address/zip"),
            Value::String("10001".to_string()),
        )
        .unwrap();

        let result = get_path(&tree, &path!("address/zip")).unwrap().unwrap();
        assert_eq!(result, &Value::String("10001".to_string()));
    }

    #[test]
    fn set_array_element() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!("scores/1"), Value::Integer(100)).unwrap();

        let result = get_path(&tree, &path!("scores/1")).unwrap().unwrap();
        assert_eq!(result, &Value::Integer(100));
    }

    #[test]
    fn set_array_append() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!("scores/3"), Value::Integer(88)).unwrap();

        let result = get_path(&tree, &path!("scores/3")).unwrap().unwrap();
        assert_eq!(result, &Value::Integer(88));
    }

    #[test]
    fn set_array_out_of_bounds_error() {
        let mut tree = test_tree();
        // scores has 3 elements (indices 0, 1, 2), appending at 3 is ok, but 5 is out of bounds
        let result = set_path(&mut tree, &path!("scores/5"), Value::Integer(99));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("out of bounds"));
    }

    #[test]
    fn set_array_invalid_index_error() {
        let mut tree = test_tree();
        let result = set_path(&mut tree, &path!("scores/abc"), Value::Integer(99));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected array index"));
    }

    #[test]
    fn set_on_primitive_error() {
        let mut tree = test_tree();
        let result = set_path(&mut tree, &path!("name/child"), Value::Integer(1));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot set child"));
    }

    #[test]
    fn set_parent_not_exists_error() {
        let mut tree = test_tree();
        let result = set_path(&mut tree, &path!("missing/deep/path"), Value::Integer(1));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn set_new_key_in_map() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!("new_key"), Value::Bool(true)).unwrap();

        let result = get_path(&tree, &path!("new_key")).unwrap().unwrap();
        assert_eq!(result, &Value::Bool(true));
    }

    #[test]
    fn set_deeply_nested() {
        let mut tree = test_tree();
        // First create the intermediate path
        set_path(
            &mut tree,
            &path!("address/state"),
            Value::String("NY".to_string()),
        )
        .unwrap();

        let result = get_path(&tree, &path!("address/state")).unwrap().unwrap();
        assert_eq!(result, &Value::String("NY".to_string()));
    }

    #[test]
    fn set_replaces_nested_structure() {
        let mut tree = test_tree();
        // Replace the entire address map with a string
        set_path(
            &mut tree,
            &path!("address"),
            Value::String("unknown".to_string()),
        )
        .unwrap();

        let result = get_path(&tree, &path!("address")).unwrap().unwrap();
        assert_eq!(result, &Value::String("unknown".to_string()));
    }

    #[test]
    fn set_array_first_element() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!("scores/0"), Value::Integer(999)).unwrap();

        let result = get_path(&tree, &path!("scores/0")).unwrap().unwrap();
        assert_eq!(result, &Value::Integer(999));
    }

    #[test]
    fn set_array_last_element() {
        let mut tree = test_tree();
        set_path(&mut tree, &path!("scores/2"), Value::Integer(777)).unwrap();

        let result = get_path(&tree, &path!("scores/2")).unwrap().unwrap();
        assert_eq!(result, &Value::Integer(777));
    }
}
