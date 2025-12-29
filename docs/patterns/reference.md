# The Reference Pattern

A **reference** is a placeholder for a value located at another path. References
enable lazy loading, pagination, and shallow responses while maintaining
consistency with the underlying data model.

## Structure

A reference is a struct with a required `path` field and an optional `type` field:

```json
{"path": "handles/0"}
```

Minimal reference. Follow the path to get the value.

```json
{
  "path": "handles/0",
  "type": {"name": "handle"}
}
```

Reference with type hint. The `name` tells clients what kind of value to expect.

```json
{
  "path": "handles/0",
  "type": {
    "name": "handle",
    "schema": {
      "position": {"type": {"name": "integer"}},
      "encoding": {"type": {"name": "string"}}
    }
  }
}
```

Reference with full type schema. The schema describes the shape of the value
without fetching it.

## Fields

### `path` (required)

The StructFS path to the actual value. Always a valid path string.

### `type` (optional)

Type information about the referenced value:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Type name (e.g., "integer", "string", "handle", "stream") |
| `schema` | map | Field names to type descriptors (recursive) |

## Detection

A value is a reference if it's a map containing a `path` key whose value is a
string. The `type` field, if present, confirms it's a reference and provides
type information.

```rust
fn is_reference(value: &Value) -> bool {
    match value {
        Value::Map(m) => {
            matches!(m.get("path"), Some(Value::String(_)))
        }
        _ => false,
    }
}
```

## Use Cases

### Shallow Collection Responses

Instead of embedding full values:

```json
{
  "items": [
    {"path": "handles/0", "type": {"name": "handle"}},
    {"path": "handles/1", "type": {"name": "handle"}},
    {"path": "handles/2", "type": {"name": "handle"}}
  ]
}
```

Clients follow references they need.

### Pagination

```json
{
  "items": [
    {"path": "handles/0"},
    {"path": "handles/1"}
  ],
  "next": {"path": "handles/after/1"}
}
```

The `next` field is itself a reference. Follow it to get the next page.

### Streaming

```json
{
  "data": "...",
  "next": {"path": "handles/0/at/4096/len/4096"}
}
```

Chunked data with a reference to the next chunk.

### Consistency Without Deep Trees

Reading a parent can return references to children:

```json
{
  "position": {"path": "meta/handles/0/position", "type": {"name": "integer"}},
  "encoding": {"path": "meta/handles/0/encoding", "type": {"name": "string"}}
}
```

The parent response is shallow. Following each reference returns the actual
value. Consistency is preserved: the value at the referenced path equals what
you'd get by reading that path directly.

## What References Are Not

References are **not** for store-specific metadata. Properties like `size`,
`encoding`, or `mode` belong on the value itself or its meta path, not on the
reference.

A reference answers: "where is it?" and optionally "what shape is it?"

It does not answer: "what are its properties?" Follow the reference for that.

## Rust Implementation

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A reference to a value at another path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    /// The path to the referenced value.
    pub path: String,

    /// Optional type information.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_info: Option<TypeInfo>,
}

/// Type information for a referenced value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeInfo {
    /// Type name (e.g., "integer", "string", "handle").
    pub name: String,

    /// Optional schema describing the structure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<BTreeMap<String, TypeDescriptor>>,
}

/// Describes a field's type within a schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDescriptor {
    /// The type of this field.
    #[serde(rename = "type")]
    pub type_info: TypeInfo,
}

impl Reference {
    /// Create a minimal reference.
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
            type_info: Some(TypeInfo {
                name: type_name.into(),
                schema: None,
            }),
        }
    }
}
```

### Serialization Examples

```rust
// Minimal
let r = Reference::new("handles/0");
// {"path": "handles/0"}

// With type
let r = Reference::with_type("handles/0", "handle");
// {"path": "handles/0", "type": {"name": "handle"}}
```

### Converting to/from Value

```rust
impl Reference {
    /// Convert to a Value::Map representation.
    pub fn to_value(&self) -> Value {
        let mut map = BTreeMap::new();
        map.insert("path".to_string(), Value::String(self.path.clone()));

        if let Some(ref ti) = self.type_info {
            map.insert("type".to_string(), ti.to_value());
        }

        Value::Map(map)
    }

    /// Try to parse a Reference from a Value.
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
}
```

## Integration with Stores

Stores return references by constructing `Reference` values and converting them
to `Value::Map`. Clients detect references by checking for the `path` key.

No changes to the `Value` enum are required. References are a convention over
the existing map type, not a new primitive.
