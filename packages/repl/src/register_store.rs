//! Register store for the REPL.
//!
//! Registers are named storage locations that can hold JSON values.
//! They can be accessed as paths starting with `@`:
//!
//! - `@foo` - The register named "foo"
//! - `@foo/bar/baz` - Navigate into the JSON structure stored in "foo"
//!
//! ## Usage
//!
//! ```text
//! @result read /some/path       # Store output in register "result"
//! read @result                   # Read the register
//! read @result/nested/field      # Read a sub-path within the register
//! write /dest @result            # Write register contents to a path
//! write @temp {"key": "value"}   # Write directly to a register
//! ```

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value as JsonValue;

use structfs_store::{Error as StoreError, Path, Reader, Writer};

/// A store that holds named registers containing JSON values.
pub struct RegisterStore {
    registers: HashMap<String, JsonValue>,
}

impl RegisterStore {
    pub fn new() -> Self {
        Self {
            registers: HashMap::new(),
        }
    }

    /// Get a register value by name.
    pub fn get(&self, name: &str) -> Option<&JsonValue> {
        self.registers.get(name)
    }

    /// Set a register value.
    pub fn set(&mut self, name: &str, value: JsonValue) {
        self.registers.insert(name.to_string(), value);
    }

    /// List all register names.
    pub fn list(&self) -> Vec<&String> {
        self.registers.keys().collect()
    }

    /// Navigate into a JSON value by path.
    fn navigate<'a>(value: &'a JsonValue, path: &Path) -> Option<&'a JsonValue> {
        let mut current = value;
        for component in path.iter() {
            current = match current {
                JsonValue::Object(map) => map.get(component)?,
                JsonValue::Array(arr) => {
                    let index: usize = component.parse().ok()?;
                    arr.get(index)?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    /// Navigate into a JSON value by path (mutable).
    fn navigate_mut<'a>(value: &'a mut JsonValue, path: &Path) -> Option<&'a mut JsonValue> {
        let mut current = value;
        for component in path.iter() {
            current = match current {
                JsonValue::Object(map) => map.get_mut(component)?,
                JsonValue::Array(arr) => {
                    let index: usize = component.parse().ok()?;
                    arr.get_mut(index)?
                }
                _ => return None,
            };
        }
        Some(current)
    }

    /// Set a value at a path within a register.
    fn set_at_path(
        value: &mut JsonValue,
        path: &Path,
        new_value: JsonValue,
    ) -> Result<(), StoreError> {
        if path.is_empty() {
            *value = new_value;
            return Ok(());
        }

        let parent_path = path.slice_as_path(0, path.len() - 1);
        let last_component = &path.components[path.len() - 1];

        let parent = if parent_path.is_empty() {
            value
        } else {
            Self::navigate_mut(value, &parent_path).ok_or_else(|| StoreError::Raw {
                message: format!("Path '{}' does not exist in register", parent_path),
            })?
        };

        match parent {
            JsonValue::Object(map) => {
                map.insert(last_component.clone(), new_value);
                Ok(())
            }
            JsonValue::Array(arr) => {
                let index: usize = last_component.parse().map_err(|_| StoreError::Raw {
                    message: format!("Invalid array index: {}", last_component),
                })?;
                if index < arr.len() {
                    arr[index] = new_value;
                    Ok(())
                } else if index == arr.len() {
                    arr.push(new_value);
                    Ok(())
                } else {
                    Err(StoreError::Raw {
                        message: format!("Array index {} out of bounds", index),
                    })
                }
            }
            _ => Err(StoreError::Raw {
                message: "Cannot set value on non-container type".to_string(),
            }),
        }
    }
}

impl Default for RegisterStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader for RegisterStore {
    fn read_to_deserializer<'de, 'this>(
        &'this mut self,
        from: &Path,
    ) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, StoreError>
    where
        'this: 'de,
    {
        if from.is_empty() {
            // List all registers
            let list: Vec<&String> = self.registers.keys().collect();
            let json =
                serde_json::to_value(&list).map_err(|e| StoreError::RecordSerialization {
                    message: e.to_string(),
                })?;
            return Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
                json,
            ))));
        }

        let register_name = &from.components[0];
        let sub_path = from.slice_as_path(1, from.len());

        let register_value = match self.registers.get(register_name) {
            Some(v) => v,
            None => return Ok(None),
        };

        let value = if sub_path.is_empty() {
            register_value.clone()
        } else {
            match Self::navigate(register_value, &sub_path) {
                Some(v) => v.clone(),
                None => return Ok(None),
            }
        };

        Ok(Some(Box::new(<dyn erased_serde::Deserializer>::erase(
            value,
        ))))
    }

    fn read_owned<RecordType: DeserializeOwned>(
        &mut self,
        from: &Path,
    ) -> Result<Option<RecordType>, StoreError> {
        if from.is_empty() {
            // List all registers
            let list: Vec<&String> = self.registers.keys().collect();
            let json =
                serde_json::to_value(&list).map_err(|e| StoreError::RecordSerialization {
                    message: e.to_string(),
                })?;
            let record =
                serde_json::from_value(json).map_err(|e| StoreError::RecordDeserialization {
                    message: e.to_string(),
                })?;
            return Ok(Some(record));
        }

        let register_name = &from.components[0];
        let sub_path = from.slice_as_path(1, from.len());

        let register_value = match self.registers.get(register_name) {
            Some(v) => v,
            None => return Ok(None),
        };

        let value = if sub_path.is_empty() {
            register_value.clone()
        } else {
            match Self::navigate(register_value, &sub_path) {
                Some(v) => v.clone(),
                None => return Ok(None),
            }
        };

        let record =
            serde_json::from_value(value).map_err(|e| StoreError::RecordDeserialization {
                message: e.to_string(),
            })?;
        Ok(Some(record))
    }
}

impl Writer for RegisterStore {
    fn write<RecordType: Serialize>(
        &mut self,
        destination: &Path,
        data: RecordType,
    ) -> Result<Path, StoreError> {
        if destination.is_empty() {
            return Err(StoreError::Raw {
                message: "Cannot write to register root. Use @name to specify a register."
                    .to_string(),
            });
        }

        let json = serde_json::to_value(&data).map_err(|e| StoreError::RecordSerialization {
            message: e.to_string(),
        })?;

        let register_name = &destination.components[0];
        let sub_path = destination.slice_as_path(1, destination.len());

        if sub_path.is_empty() {
            // Writing to the register itself
            self.registers.insert(register_name.clone(), json);
        } else {
            // Writing to a sub-path within the register
            let register_value = self
                .registers
                .entry(register_name.clone())
                .or_insert_with(|| JsonValue::Object(serde_json::Map::new()));
            Self::set_at_path(register_value, &sub_path, json)?;
        }

        Ok(destination.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use structfs_store::path;

    #[test]
    fn test_basic_read_write() {
        let mut store = RegisterStore::new();
        store.write(&path!("foo"), json!({"bar": 123})).unwrap();
        let result: JsonValue = store.read_owned(&path!("foo")).unwrap().unwrap();
        assert_eq!(result, json!({"bar": 123}));
    }

    #[test]
    fn test_sub_path_read() {
        let mut store = RegisterStore::new();
        store
            .write(&path!("foo"), json!({"bar": {"baz": 456}}))
            .unwrap();
        let result: JsonValue = store.read_owned(&path!("foo/bar/baz")).unwrap().unwrap();
        assert_eq!(result, json!(456));
    }

    #[test]
    fn test_list_registers() {
        let mut store = RegisterStore::new();
        store.write(&path!("foo"), json!(1)).unwrap();
        store.write(&path!("bar"), json!(2)).unwrap();
        let list: Vec<String> = store.read_owned(&path!("")).unwrap().unwrap();
        assert!(list.contains(&"foo".to_string()));
        assert!(list.contains(&"bar".to_string()));
    }
}
