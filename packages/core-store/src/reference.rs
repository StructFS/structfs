//! The Reference pattern - a placeholder for a value at another path.
//!
//! References enable lazy loading, pagination, and shallow responses while
//! maintaining consistency with the underlying data model. They are the key
//! to HATEOAS in StructFS: clients discover and navigate the API by following
//! embedded references, not by constructing paths from out-of-band knowledge.

use std::collections::BTreeMap;

use crate::Value;

/// Type information for a referenced value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    /// Type name (e.g., "integer", "string", "handle", "action", "collection").
    pub name: String,

    /// Optional schema describing the structure of the type.
    pub schema: Option<BTreeMap<String, TypeDescriptor>>,
}

impl TypeInfo {
    /// Create a TypeInfo with just a name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
        }
    }

    /// Convert to a Value::Map representation.
    pub fn to_value(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Value::String(self.name.clone()));

        if let Some(ref schema) = self.schema {
            let schema_map: BTreeMap<String, Value> = schema
                .iter()
                .map(|(k, v)| (k.clone(), v.to_value()))
                .collect();
            map.insert("schema".to_string(), Value::Map(schema_map));
        }

        Value::Map(map)
    }

    /// Try to parse a TypeInfo from a Value.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = match value {
            Value::Map(m) => m,
            _ => return None,
        };

        let name = match map.get("name") {
            Some(Value::String(s)) => s.clone(),
            _ => return None,
        };

        let schema = map.get("schema").and_then(|v| {
            if let Value::Map(schema_map) = v {
                let mut result = BTreeMap::new();
                for (k, v) in schema_map {
                    if let Some(td) = TypeDescriptor::from_value(v) {
                        result.insert(k.clone(), td);
                    }
                }
                Some(result)
            } else {
                None
            }
        });

        Some(Self { name, schema })
    }
}

/// Describes a field's type within a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDescriptor {
    /// The type of this field.
    pub type_info: TypeInfo,
}

impl TypeDescriptor {
    /// Create a TypeDescriptor from a type name.
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_info: TypeInfo::new(type_name),
        }
    }

    /// Convert to a Value::Map representation.
    pub fn to_value(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert("type".to_string(), self.type_info.to_value());
        Value::Map(map)
    }

    /// Try to parse a TypeDescriptor from a Value.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = match value {
            Value::Map(m) => m,
            _ => return None,
        };

        let type_info = map.get("type").and_then(TypeInfo::from_value)?;

        Some(Self { type_info })
    }
}

/// A reference to a value at another path.
///
/// References are the foundation of HATEOAS in StructFS. Instead of embedding
/// full values or documenting path construction rules, stores return references
/// that clients can follow.
///
/// # Examples
///
/// ```
/// use structfs_core_store::Reference;
///
/// // Minimal reference
/// let r = Reference::new("handles/0");
///
/// // Reference with type hint
/// let r = Reference::with_type("handles/0", "handle");
///
/// // Reference to an action
/// let r = Reference::with_type("meta/open", "action");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The path to the referenced value.
    pub path: String,

    /// Optional type information.
    pub type_info: Option<TypeInfo>,
}

impl Reference {
    /// Create a minimal reference with just a path.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            type_info: None,
        }
    }

    /// Create a reference with a type name.
    pub fn with_type(path: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            type_info: Some(TypeInfo::new(type_name)),
        }
    }

    /// Convert to a Value::Map representation.
    ///
    /// This produces the canonical reference format:
    /// ```json
    /// {"path": "handles/0", "type": {"name": "handle"}}
    /// ```
    pub fn to_value(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::String(self.path.clone()));

        if let Some(ref ti) = self.type_info {
            map.insert("type".to_string(), ti.to_value());
        }

        Value::Map(map)
    }

    /// Try to parse a Reference from a Value.
    ///
    /// A value is a reference if it's a map containing a `path` key whose
    /// value is a string.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = match value {
            Value::Map(m) => m,
            _ => return None,
        };

        let path = match map.get("path") {
            Some(Value::String(s)) => s.clone(),
            _ => return None,
        };

        let type_info = map.get("type").and_then(TypeInfo::from_value);

        Some(Self { path, type_info })
    }

    /// Check if a Value is a reference.
    ///
    /// A value is a reference if it's a map containing a `path` key whose
    /// value is a string.
    pub fn is_reference(value: &Value) -> bool {
        match value {
            Value::Map(m) => matches!(m.get("path"), Some(Value::String(_))),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_new() {
        let r = Reference::new("handles/0");
        assert_eq!(r.path, "handles/0");
        assert!(r.type_info.is_none());
    }

    #[test]
    fn reference_with_type() {
        let r = Reference::with_type("handles/0", "handle");
        assert_eq!(r.path, "handles/0");
        assert_eq!(r.type_info.as_ref().unwrap().name, "handle");
    }

    #[test]
    fn reference_to_value_minimal() {
        let r = Reference::new("handles/0");
        let v = r.to_value();

        if let Value::Map(map) = v {
            assert_eq!(map.get("path"), Some(&Value::String("handles/0".into())));
            assert!(!map.contains_key("type"));
        } else {
            panic!("Expected map");
        }
    }

    #[test]
    fn reference_to_value_with_type() {
        let r = Reference::with_type("handles/0", "handle");
        let v = r.to_value();

        if let Value::Map(map) = v {
            assert_eq!(map.get("path"), Some(&Value::String("handles/0".into())));
            if let Some(Value::Map(type_map)) = map.get("type") {
                assert_eq!(type_map.get("name"), Some(&Value::String("handle".into())));
            } else {
                panic!("Expected type map");
            }
        } else {
            panic!("Expected map");
        }
    }

    #[test]
    fn reference_from_value_minimal() {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::String("handles/0".into()));
        let v = Value::Map(map);

        let r = Reference::from_value(&v).unwrap();
        assert_eq!(r.path, "handles/0");
        assert!(r.type_info.is_none());
    }

    #[test]
    fn reference_from_value_with_type() {
        let mut type_map = BTreeMap::new();
        type_map.insert("name".to_string(), Value::String("handle".into()));

        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::String("handles/0".into()));
        map.insert("type".to_string(), Value::Map(type_map));
        let v = Value::Map(map);

        let r = Reference::from_value(&v).unwrap();
        assert_eq!(r.path, "handles/0");
        assert_eq!(r.type_info.as_ref().unwrap().name, "handle");
    }

    #[test]
    fn reference_from_value_not_map() {
        assert!(Reference::from_value(&Value::String("x".into())).is_none());
    }

    #[test]
    fn reference_from_value_no_path() {
        let mut map = BTreeMap::new();
        map.insert("other".to_string(), Value::String("x".into()));
        assert!(Reference::from_value(&Value::Map(map)).is_none());
    }

    #[test]
    fn reference_from_value_path_not_string() {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::Integer(42));
        assert!(Reference::from_value(&Value::Map(map)).is_none());
    }

    #[test]
    fn is_reference_true() {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::String("x".into()));
        assert!(Reference::is_reference(&Value::Map(map)));
    }

    #[test]
    fn is_reference_false_not_map() {
        assert!(!Reference::is_reference(&Value::String("x".into())));
    }

    #[test]
    fn is_reference_false_no_path() {
        let mut map = BTreeMap::new();
        map.insert("other".to_string(), Value::String("x".into()));
        assert!(!Reference::is_reference(&Value::Map(map)));
    }

    #[test]
    fn is_reference_false_path_not_string() {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::Integer(42));
        assert!(!Reference::is_reference(&Value::Map(map)));
    }

    #[test]
    fn reference_roundtrip() {
        let original = Reference::with_type("meta/handles/0", "handle");
        let value = original.to_value();
        let parsed = Reference::from_value(&value).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn type_info_new() {
        let ti = TypeInfo::new("handle");
        assert_eq!(ti.name, "handle");
        assert!(ti.schema.is_none());
    }

    #[test]
    fn type_info_to_value() {
        let ti = TypeInfo::new("integer");
        let v = ti.to_value();

        if let Value::Map(map) = v {
            assert_eq!(map.get("name"), Some(&Value::String("integer".into())));
        } else {
            panic!("Expected map");
        }
    }

    #[test]
    fn type_info_with_schema() {
        let mut schema = BTreeMap::new();
        schema.insert("position".to_string(), TypeDescriptor::new("integer"));

        let ti = TypeInfo {
            name: "handle".to_string(),
            schema: Some(schema),
        };

        let v = ti.to_value();
        if let Value::Map(map) = v {
            assert!(map.contains_key("schema"));
        } else {
            panic!("Expected map");
        }
    }

    #[test]
    fn type_info_from_value() {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Value::String("handle".into()));
        let v = Value::Map(map);

        let ti = TypeInfo::from_value(&v).unwrap();
        assert_eq!(ti.name, "handle");
    }

    #[test]
    fn type_info_from_value_not_map() {
        assert!(TypeInfo::from_value(&Value::String("x".into())).is_none());
    }

    #[test]
    fn type_info_from_value_no_name() {
        let map = BTreeMap::new();
        assert!(TypeInfo::from_value(&Value::Map(map)).is_none());
    }

    #[test]
    fn type_descriptor_new() {
        let td = TypeDescriptor::new("string");
        assert_eq!(td.type_info.name, "string");
    }

    #[test]
    fn type_descriptor_to_value() {
        let td = TypeDescriptor::new("integer");
        let v = td.to_value();

        if let Value::Map(map) = v {
            assert!(map.contains_key("type"));
        } else {
            panic!("Expected map");
        }
    }

    #[test]
    fn type_descriptor_from_value() {
        let mut type_map = BTreeMap::new();
        type_map.insert("name".to_string(), Value::String("integer".into()));

        let mut map = BTreeMap::new();
        map.insert("type".to_string(), Value::Map(type_map));

        let td = TypeDescriptor::from_value(&Value::Map(map)).unwrap();
        assert_eq!(td.type_info.name, "integer");
    }

    #[test]
    fn type_descriptor_from_value_not_map() {
        assert!(TypeDescriptor::from_value(&Value::String("x".into())).is_none());
    }

    #[test]
    fn type_descriptor_from_value_no_type() {
        let map = BTreeMap::new();
        assert!(TypeDescriptor::from_value(&Value::Map(map)).is_none());
    }
}
