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
}
