//! The Value type - a tree-shaped data structure.
//!
//! This is the "struct" in StructFS. It's a dynamically-typed tree that can
//! represent any structured data: JSON, MessagePack, CBOR, protobuf (with schema), etc.

use std::collections::BTreeMap;

use crate::{Error, Path};

/// A tree-shaped value that can be read from or written to a Store.
///
/// This is the universal data representation in StructFS. It maps directly
/// to JSON, MessagePack, CBOR, etc., but is encoding-agnostic.
///
/// # Design Notes
///
/// - Uses `BTreeMap` for deterministic ordering (important for hashing, comparison)
/// - Includes `Bytes` for binary data (unlike JSON, but like CBOR/MessagePack)
/// - Uses `i64` for integers (sufficient for most use cases, matches many protocols)
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    /// Absence of a value. Distinct from "path doesn't exist".
    #[default]
    Null,
    /// Boolean value.
    Bool(bool),
    /// Signed 64-bit integer.
    Integer(i64),
    /// 64-bit floating point.
    Float(f64),
    /// UTF-8 string.
    String(String),
    /// Binary data (for formats that support it: CBOR, MessagePack, etc.)
    Bytes(Vec<u8>),
    /// Ordered sequence of values.
    Array(Vec<Value>),
    /// Key-value map with string keys (the "struct" part).
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// Create a null value.
    pub fn null() -> Self {
        Value::Null
    }

    /// Create an empty map.
    pub fn map() -> Self {
        Value::Map(BTreeMap::new())
    }

    /// Create an empty array.
    pub fn array() -> Self {
        Value::Array(Vec::new())
    }

    /// Check if this value is null.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Check if this value is a map (struct).
    pub fn is_map(&self) -> bool {
        matches!(self, Value::Map(_))
    }

    /// Check if this value is an array.
    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }

    /// Get a reference to a nested value by path.
    ///
    /// Returns `None` if the path doesn't exist or can't be navigated
    /// (e.g., trying to index into a string).
    pub fn get(&self, path: &Path) -> Option<&Value> {
        let mut current = self;
        for component in path.iter() {
            current = match current {
                Value::Map(map) => map.get(component)?,
                Value::Array(arr) => {
                    let index: usize = component.parse().ok()?;
                    arr.get(index)?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    /// Get a mutable reference to a nested value by path.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut Value> {
        let mut current = self;
        for component in path.iter() {
            current = match current {
                Value::Map(map) => map.get_mut(component)?,
                Value::Array(arr) => {
                    let index: usize = component.parse().ok()?;
                    arr.get_mut(index)?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    /// Set a value at a path, creating intermediate maps as needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the path traverses through a non-container value
    /// (e.g., trying to set `foo/bar` when `foo` is a string).
    pub fn set(&mut self, path: &Path, value: Value) -> Result<(), Error> {
        if path.is_empty() {
            *self = value;
            return Ok(());
        }

        let mut current = self;

        // Navigate to parent, creating intermediate maps
        for (i, component) in path.iter().enumerate() {
            let is_last = i == path.len() - 1;

            if is_last {
                // Set the value at the last component
                match current {
                    Value::Map(map) => {
                        map.insert(component.clone(), value);
                        return Ok(());
                    }
                    Value::Array(arr) => {
                        let index: usize = component.parse().map_err(|_| Error::InvalidPath {
                            message: format!("invalid array index: {}", component),
                        })?;
                        if index < arr.len() {
                            arr[index] = value;
                        } else if index == arr.len() {
                            arr.push(value);
                        } else {
                            return Err(Error::InvalidPath {
                                message: format!("array index {} out of bounds", index),
                            });
                        }
                        return Ok(());
                    }
                    _ => {
                        return Err(Error::InvalidPath {
                            message: format!(
                                "cannot set child '{}' on non-container value",
                                component
                            ),
                        });
                    }
                }
            } else {
                // Navigate or create intermediate map
                match current {
                    Value::Map(map) => {
                        current = map
                            .entry(component.clone())
                            .or_insert_with(|| Value::Map(BTreeMap::new()));
                    }
                    Value::Array(arr) => {
                        let index: usize = component.parse().map_err(|_| Error::InvalidPath {
                            message: format!("invalid array index: {}", component),
                        })?;
                        current = arr.get_mut(index).ok_or_else(|| Error::InvalidPath {
                            message: format!("array index {} out of bounds", index),
                        })?;
                    }
                    _ => {
                        return Err(Error::InvalidPath {
                            message: format!(
                                "cannot navigate through non-container at '{}'",
                                component
                            ),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// Remove a value at a path, returning it if it existed.
    pub fn remove(&mut self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let old = std::mem::replace(self, Value::Null);
            return Ok(Some(old));
        }

        // Navigate to parent
        let parent_path = Path {
            components: path.components[..path.len() - 1].to_vec(),
        };
        let last_component = &path.components[path.len() - 1];

        let parent = match self.get_mut(&parent_path) {
            Some(p) => p,
            None => return Ok(None),
        };

        match parent {
            Value::Map(map) => Ok(map.remove(last_component)),
            Value::Array(arr) => {
                let index: usize = last_component.parse().map_err(|_| Error::InvalidPath {
                    message: format!("invalid array index: {}", last_component),
                })?;
                if index < arr.len() {
                    Ok(Some(arr.remove(index)))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }
}

// Conversion from common types

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Integer(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Integer(v as i64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}

impl From<Vec<u8>> for Value {
    fn from(v: Vec<u8>) -> Self {
        Value::Bytes(v)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path;

    #[test]
    fn get_nested_value() {
        let mut value = Value::map();
        value.set(&path!("foo/bar"), Value::from("hello")).unwrap();

        assert_eq!(value.get(&path!("foo/bar")), Some(&Value::from("hello")));
        // foo should contain bar, not be empty
        let foo = value.get(&path!("foo")).unwrap();
        assert!(foo.is_map());
        assert_eq!(foo.get(&path!("bar")), Some(&Value::from("hello")));
        assert_eq!(value.get(&path!("nonexistent")), None);
    }

    #[test]
    fn set_creates_intermediate_maps() {
        let mut value = Value::map();
        value.set(&path!("a/b/c/d"), Value::from(42i64)).unwrap();

        assert_eq!(value.get(&path!("a/b/c/d")), Some(&Value::from(42i64)));
        assert!(value.get(&path!("a")).unwrap().is_map());
        assert!(value.get(&path!("a/b")).unwrap().is_map());
    }

    #[test]
    fn remove_works() {
        let mut value = Value::map();
        value.set(&path!("foo/bar"), Value::from("hello")).unwrap();

        let removed = value.remove(&path!("foo/bar")).unwrap();
        assert_eq!(removed, Some(Value::from("hello")));
        assert_eq!(value.get(&path!("foo/bar")), None);

        // Parent still exists
        assert!(value.get(&path!("foo")).is_some());
    }

    #[test]
    fn array_access_works() {
        let mut value = Value::map();
        value
            .set(
                &path!("items"),
                Value::Array(vec![Value::from("a"), Value::from("b"), Value::from("c")]),
            )
            .unwrap();

        assert_eq!(value.get(&path!("items/0")), Some(&Value::from("a")));
        assert_eq!(value.get(&path!("items/1")), Some(&Value::from("b")));
        assert_eq!(value.get(&path!("items/2")), Some(&Value::from("c")));
        assert_eq!(value.get(&path!("items/3")), None);
    }

    #[test]
    fn value_constructors() {
        assert!(Value::null().is_null());
        assert!(Value::map().is_map());
        assert!(Value::array().is_array());
    }

    #[test]
    fn value_default_is_null() {
        let value = Value::default();
        assert!(value.is_null());
    }

    #[test]
    fn value_type_checks() {
        assert!(Value::Null.is_null());
        assert!(!Value::Null.is_map());
        assert!(!Value::Null.is_array());

        assert!(!Value::Bool(true).is_null());
        assert!(!Value::Bool(true).is_map());
        assert!(!Value::Bool(true).is_array());

        assert!(Value::Map(BTreeMap::new()).is_map());
        assert!(!Value::Map(BTreeMap::new()).is_null());

        assert!(Value::Array(vec![]).is_array());
        assert!(!Value::Array(vec![]).is_null());
    }

    #[test]
    fn get_on_empty_path_returns_self() {
        let value = Value::from("hello");
        assert_eq!(value.get(&path!("")), Some(&value));
    }

    #[test]
    fn get_on_primitive_returns_none() {
        let value = Value::from("hello");
        assert_eq!(value.get(&path!("foo")), None);
    }

    #[test]
    fn get_mut_works() {
        let mut value = Value::map();
        value.set(&path!("foo"), Value::from("bar")).unwrap();

        let foo = value.get_mut(&path!("foo")).unwrap();
        *foo = Value::from("baz");

        assert_eq!(value.get(&path!("foo")), Some(&Value::from("baz")));
    }

    #[test]
    fn get_mut_on_array() {
        let mut value = Value::Array(vec![Value::from(1i64), Value::from(2i64)]);

        let first = value.get_mut(&path!("0")).unwrap();
        *first = Value::from(100i64);

        assert_eq!(value.get(&path!("0")), Some(&Value::from(100i64)));
    }

    #[test]
    fn get_mut_on_primitive_returns_none() {
        let mut value = Value::from("hello");
        assert!(value.get_mut(&path!("foo")).is_none());
    }

    #[test]
    fn set_at_empty_path_replaces_self() {
        let mut value = Value::from("old");
        value.set(&path!(""), Value::from("new")).unwrap();
        assert_eq!(value, Value::from("new"));
    }

    #[test]
    fn set_array_element() {
        let mut value = Value::Array(vec![Value::from("a"), Value::from("b")]);
        value.set(&path!("0"), Value::from("x")).unwrap();
        assert_eq!(value.get(&path!("0")), Some(&Value::from("x")));
    }

    #[test]
    fn set_array_append() {
        let mut value = Value::Array(vec![Value::from("a")]);
        value.set(&path!("1"), Value::from("b")).unwrap();
        assert_eq!(value.get(&path!("1")), Some(&Value::from("b")));
    }

    #[test]
    fn set_array_out_of_bounds_error() {
        let mut value = Value::Array(vec![Value::from("a")]);
        let result = value.set(&path!("5"), Value::from("x"));
        assert!(result.is_err());
    }

    #[test]
    fn set_on_primitive_error() {
        let mut value = Value::from("hello");
        let result = value.set(&path!("foo"), Value::from("bar"));
        assert!(result.is_err());
    }

    #[test]
    fn set_invalid_array_index_error() {
        let mut value = Value::Array(vec![Value::from("a")]);
        let result = value.set(&path!("not_a_number"), Value::from("x"));
        assert!(result.is_err());
    }

    #[test]
    fn set_through_array() {
        let mut value = Value::map();
        value
            .set(
                &path!("items"),
                Value::Array(vec![Value::map(), Value::map()]),
            )
            .unwrap();
        value
            .set(&path!("items/0/name"), Value::from("first"))
            .unwrap();

        assert_eq!(
            value.get(&path!("items/0/name")),
            Some(&Value::from("first"))
        );
    }

    #[test]
    fn set_through_array_invalid_index_error() {
        let mut value = Value::map();
        value.set(&path!("items"), Value::Array(vec![])).unwrap();
        let result = value.set(&path!("items/0/name"), Value::from("x"));
        assert!(result.is_err());
    }

    #[test]
    fn set_through_primitive_error() {
        let mut value = Value::map();
        value.set(&path!("foo"), Value::from("primitive")).unwrap();
        let result = value.set(&path!("foo/bar"), Value::from("x"));
        assert!(result.is_err());
    }

    #[test]
    fn remove_at_empty_path() {
        let mut value = Value::from("hello");
        let removed = value.remove(&path!("")).unwrap();
        assert_eq!(removed, Some(Value::from("hello")));
        assert!(value.is_null());
    }

    #[test]
    fn remove_nonexistent() {
        let mut value = Value::map();
        let removed = value.remove(&path!("nonexistent")).unwrap();
        assert_eq!(removed, None);
    }

    #[test]
    fn remove_from_array() {
        let mut value = Value::Array(vec![Value::from("a"), Value::from("b"), Value::from("c")]);
        let removed = value.remove(&path!("1")).unwrap();
        assert_eq!(removed, Some(Value::from("b")));

        // Array should now be [a, c]
        match &value {
            Value::Array(arr) => assert_eq!(arr.len(), 2),
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn remove_from_array_out_of_bounds() {
        let mut value = Value::Array(vec![Value::from("a")]);
        let removed = value.remove(&path!("5")).unwrap();
        assert_eq!(removed, None);
    }

    #[test]
    fn remove_invalid_array_index_error() {
        let mut value = Value::Array(vec![Value::from("a")]);
        let result = value.remove(&path!("not_a_number"));
        assert!(result.is_err());
    }

    #[test]
    fn remove_from_primitive() {
        let mut value = Value::map();
        value.set(&path!("foo"), Value::from("primitive")).unwrap();
        let removed = value.remove(&path!("foo/bar")).unwrap();
        assert_eq!(removed, None);
    }

    #[test]
    fn from_bool() {
        assert_eq!(Value::from(true), Value::Bool(true));
        assert_eq!(Value::from(false), Value::Bool(false));
    }

    #[test]
    fn from_i64() {
        assert_eq!(Value::from(42i64), Value::Integer(42));
        assert_eq!(Value::from(-100i64), Value::Integer(-100));
    }

    #[test]
    fn from_i32() {
        assert_eq!(Value::from(42i32), Value::Integer(42));
    }

    #[test]
    fn from_f64() {
        assert_eq!(Value::from(2.75f64), Value::Float(2.75));
    }

    #[test]
    fn from_string() {
        assert_eq!(
            Value::from("hello".to_string()),
            Value::String("hello".to_string())
        );
    }

    #[test]
    fn from_str() {
        assert_eq!(Value::from("hello"), Value::String("hello".to_string()));
    }

    #[test]
    fn from_vec_u8() {
        assert_eq!(Value::from(vec![1u8, 2, 3]), Value::Bytes(vec![1u8, 2, 3]));
    }

    #[test]
    fn from_vec_values() {
        let values: Vec<i64> = vec![1, 2, 3];
        let value = Value::from(values);
        match value {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], Value::Integer(1));
            }
            _ => panic!("Expected array"),
        }
    }

    #[test]
    fn value_equality() {
        assert_eq!(Value::Null, Value::Null);
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_ne!(Value::Bool(true), Value::Bool(false));
        assert_eq!(Value::Integer(42), Value::Integer(42));
        assert_ne!(Value::Integer(42), Value::Integer(43));
        assert_eq!(
            Value::String("a".to_string()),
            Value::String("a".to_string())
        );
        assert_ne!(
            Value::String("a".to_string()),
            Value::String("b".to_string())
        );
    }

    #[test]
    fn value_clone() {
        let original = Value::Map({
            let mut m = BTreeMap::new();
            m.insert("key".to_string(), Value::from("value"));
            m
        });
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn value_debug() {
        let value = Value::from("test");
        let debug = format!("{:?}", value);
        assert!(debug.contains("String"));
        assert!(debug.contains("test"));
    }
}
