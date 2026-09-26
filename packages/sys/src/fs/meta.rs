//! The `meta/...` lens: machine-readable affordances for the fs store.

use std::io::Seek;

use collection_literals::btree;
use structfs_core_store::{Error, Path, Reference, Value};

use super::encoding::{non_negative, ContentEncoding, OpenMode};
use super::handles::FileHandle;
use super::FsStore;

/// An accepted request field: (name, type, required).
type Field = (&'static str, &'static str, bool);

/// The path actions, with their `accepts` fields.
const ACTIONS: &[(&str, &[Field])] = &[
    (
        "open",
        &[
            ("path", "string", true),
            ("mode", "string", false),
            ("encoding", "string", false),
        ],
    ),
    ("stat", &[("path", "string", true)]),
    ("readdir", &[("path", "string", true)]),
    (
        "mkdir",
        &[("path", "string", true), ("recursive", "boolean", false)],
    ),
    ("rmdir", &[("path", "string", true)]),
    ("unlink", &[("path", "string", true)]),
    (
        "rename",
        &[("from", "string", true), ("to", "string", true)],
    ),
];

fn type_name(name: &str) -> Value {
    Value::Map(btree! { "name".into() => Value::String(name.into()) })
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|s| Value::String((*s).into())).collect())
}

fn action_schema(action: &str, fields: &[Field]) -> Value {
    let mut accepts = std::collections::BTreeMap::new();
    for (name, ty, required) in fields {
        let mut field = btree! { "type".into() => type_name(ty) };
        if *required {
            field.insert("required".into(), Value::Bool(true));
        }
        match (action, *name) {
            ("open", "mode") => {
                field.insert("values".into(), strings(OpenMode::NAMES));
            }
            ("open", "encoding") => {
                field.insert("values".into(), strings(ContentEncoding::NAMES));
            }
            _ => {}
        }
        accepts.insert((*name).to_string(), Value::Map(field));
    }
    let mut schema = btree! {
        "type".into() => type_name("action"),
        "method".into() => Value::String("write".into()),
        "target".into() => Reference::new(action).to_value(),
        "accepts".into() => Value::Map(accepts),
    };
    let returns = match action {
        "open" => Some(Value::Map(btree! {
            "type".into() => type_name("handle"),
            "collection".into() => Reference::new("handles").to_value(),
        })),
        "stat" | "readdir" => Some(Value::Map(btree! {
            "type".into() => type_name(action),
            "collection".into() => Reference::new("results").to_value(),
        })),
        _ => None,
    };
    if let Some(returns) = returns {
        schema.insert("returns".into(), returns);
    }
    Value::Map(schema)
}

fn describe(readable: bool, writable: bool, description: &str) -> Value {
    let mut map = btree! {
        "description".into() => Value::String(description.into()),
    };
    if readable {
        map.insert("readable".into(), Value::Bool(true));
    }
    if writable {
        map.insert("writable".into(), Value::Bool(true));
    }
    Value::Map(map)
}

impl FsStore {
    pub(super) fn read_meta(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let mut root = btree! {
                "type".into() => type_name("store"),
                "handles".into() => Reference::with_type("meta/handles", "collection").to_value(),
            };
            for (action, _) in ACTIONS {
                root.insert(
                    (*action).to_string(),
                    Reference::with_type(format!("meta/{action}"), "action").to_value(),
                );
            }
            return Ok(Some(Value::Map(root)));
        }
        if &path[0] == "handles" {
            return self.read_meta_handles(&path.slice(1, path.len()));
        }
        Ok(ACTIONS
            .iter()
            .find(|(action, _)| *action == &path[0])
            .and_then(|(action, fields)| {
                action_schema(action, fields)
                    .get(&path.slice(1, path.len()))
                    .cloned()
            }))
    }

    fn read_meta_handles(&self, path: &Path) -> Result<Option<Value>, Error> {
        if path.is_empty() {
            let items = self
                .handles
                .keys()
                .map(|id| Reference::with_type(format!("meta/handles/{id}"), "handle").to_value())
                .collect();
            return Ok(Some(Value::Map(btree! {
                "type".into() => type_name("collection"),
                "items".into() => Value::Array(items),
            })));
        }
        let id: u64 = path[0]
            .parse()
            .map_err(|_| Error::invalid_argument(format!("invalid handle id '{}'", &path[0])))?;
        let handle = self.handle(id)?;
        if path.len() == 1 {
            return Ok(Some(Self::meta_handle(id, handle)?));
        }
        let entry = match &path[1] {
            "position" => Some(Value::Map(btree! {
                "readable".into() => Value::Bool(true),
                "writable".into() => Value::Bool(true),
                "type".into() => Value::String("integer".into()),
                "value".into() => Value::Integer(position(handle)? as i64),
                "description".into() => Value::String("Current byte offset. Write to seek.".into()),
            })),
            "meta" => Some(describe(true, false, "File metadata (size, type)")),
            "at" => Some(describe(
                true,
                true,
                "Read/write at offset: at/{offset} or at/{offset}/len/{n}",
            )),
            "close" => Some(describe(false, true, "Close the file handle")),
            _ => None,
        };
        Ok(entry.and_then(|entry| entry.get(&path.slice(2, path.len())).cloned()))
    }

    fn meta_handle(id: u64, handle: &FileHandle) -> Result<Value, Error> {
        let meta_prefix = format!("meta/handles/{id}");
        let data_prefix = format!("handles/{id}");
        Ok(Value::Map(btree! {
            "state".into() => Value::Map(btree! {
                "position".into() => Value::Integer(position(handle)? as i64),
                "encoding".into() => Value::String(handle.encoding.name().into()),
                "mode".into() => Value::String(handle.mode.name().into()),
                "file".into() => Value::String(handle.path.clone()),
            }),
            "position".into() => Reference::with_type(format!("{meta_prefix}/position"), "integer").to_value(),
            "encoding".into() => Reference::with_type(format!("{meta_prefix}/encoding"), "string").to_value(),
            "close".into() => Reference::with_type(format!("{meta_prefix}/close"), "action").to_value(),
            "content".into() => Reference::with_type(&data_prefix, "stream").to_value(),
            "at".into() => Reference::with_type(format!("{data_prefix}/at"), "accessor").to_value(),
        }))
    }

    /// Only `meta/handles/{id}/position` is writable: write an offset to seek.
    pub(super) fn write_meta(&mut self, path: &Path, value: &Value) -> Result<(), Error> {
        let id = match path.len() {
            3 if &path[0] == "handles" && &path[2] == "position" => path[1].parse::<u64>().ok(),
            _ => None,
        }
        .ok_or_else(|| Error::permission_denied(format!("meta/{path} is not writable")))?;
        self.seek(id, non_negative(value, "position")?)
    }
}

fn position(handle: &FileHandle) -> Result<u64, Error> {
    Ok((&handle.file).stream_position()?)
}
