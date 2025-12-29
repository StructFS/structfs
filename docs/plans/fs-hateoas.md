# Plan: HATEOAS Refactor for FsStore

This plan describes refactoring FsStore to follow hypermedia principles using
the reference pattern. Clients discover state transitions by following
references in responses, not by constructing paths from out-of-band knowledge.

## Principle

Every response that refers to another resource returns a reference, not a bare
ID or path string embedded in documentation. Clients navigate by following
references.

## Changes

### 1. Root Returns References

**Before:**
```json
{
  "open": "Write {path, mode} to get handle",
  "handles": "Open file handles",
  "stat": "Write {path} to get file info",
  "mkdir": "Write {path} to create directory",
  "rmdir": "Write {path} to remove directory",
  "unlink": "Write {path} to delete file",
  "rename": "Write {from, to} to rename"
}
```

**After:**
```json
{
  "handles": {"path": "handles", "type": {"name": "collection"}},
  "open": {"path": "meta/open", "type": {"name": "action"}},
  "stat": {"path": "meta/stat", "type": {"name": "action"}},
  "mkdir": {"path": "meta/mkdir", "type": {"name": "action"}},
  "rmdir": {"path": "meta/rmdir", "type": {"name": "action"}},
  "unlink": {"path": "meta/unlink", "type": {"name": "action"}},
  "rename": {"path": "meta/rename", "type": {"name": "action"}},
  "meta": {"path": "meta", "type": {"name": "meta"}}
}
```

Every field is a reference. To learn how to open a file, follow `open`. To see
handles, follow `handles`.

### 2. Handles Collection Returns References

**Before:**
```json
[0, 1, 2]
```

**After:**
```json
{
  "items": [
    {"path": "handles/0", "type": {"name": "handle"}},
    {"path": "handles/1", "type": {"name": "handle"}},
    {"path": "handles/2", "type": {"name": "handle"}}
  ]
}
```

Clients iterate `items` and follow references. No path construction from IDs.

**With pagination:**
```json
{
  "items": [
    {"path": "handles/0", "type": {"name": "handle"}},
    {"path": "handles/1", "type": {"name": "handle"}}
  ],
  "next": {"path": "handles/after/1"}
}
```

The `next` field is a reference. Follow it to get the next page.

### 3. Handle Content Unchanged

```
read handles/0
```

Returns file content as before (string or bytes depending on encoding). This is
the data path - it returns data.

Navigation and control live in meta.

### 4. Meta Handle Returns State + References

```
read meta/handles/0
```

**Before:**
```json
{
  "readable": true,
  "writable": true,
  "state": {
    "position": 1024,
    "encoding": "utf8",
    "mode": "readwrite",
    "path": "/tmp/example.txt"
  },
  "fields": {
    "position": {"readable": true, "writable": true, "type": "integer"},
    ...
  }
}
```

**After:**
```json
{
  "state": {
    "position": 1024,
    "encoding": "utf8",
    "mode": "readwrite",
    "file": "/tmp/example.txt"
  },
  "position": {"path": "meta/handles/0/position", "type": {"name": "integer"}},
  "encoding": {"path": "meta/handles/0/encoding", "type": {"name": "string"}},
  "close": {"path": "meta/handles/0/close", "type": {"name": "action"}},
  "content": {"path": "handles/0", "type": {"name": "stream"}},
  "at": {"path": "handles/0/at", "type": {"name": "accessor"}}
}
```

The `state` field contains current values inline for display. The reference
fields tell clients where to read/write each aspect. Following `position`
returns the integer value. Writing to it seeks.

### 5. Action Descriptions in Meta

```
read meta/open
```

Returns:
```json
{
  "type": {"name": "action"},
  "method": "write",
  "target": {"path": "open"},
  "accepts": {
    "path": {"type": {"name": "string"}, "required": true},
    "mode": {
      "type": {"name": "string"},
      "values": ["read", "write", "append", "readwrite", "createnew"]
    },
    "encoding": {
      "type": {"name": "string"},
      "values": ["base64", "utf8", "bytes"]
    }
  },
  "returns": {"type": {"name": "handle"}, "collection": {"path": "handles"}}
}
```

The `target` field tells clients where to write. The `returns` field describes
what they'll get back (a handle, which lives in the handles collection).

### 6. Writer Returns Path (Unchanged)

`Writer::write` continues to return `Result<Path, Error>`. The returned path
points to the created/modified resource.

```
write open {"path": "/tmp/foo.txt", "mode": "read"}
```

Returns: `Path("handles/3")`

The client can then:
- `read handles/3` for content
- `read meta/handles/3` for navigation and control references

The path itself is the reference target. Reading it (or its meta) gives the
hypermedia-rich response.

## Implementation

### Reference Helper

Use the `Reference` struct from the reference pattern:

```rust
impl FsStore {
    fn ref_to(path: &str, type_name: &str) -> Value {
        Reference::with_type(path, type_name).to_value()
    }
}
```

### Root Response

```rust
fn read_root(&self) -> Value {
    let mut map = BTreeMap::new();
    map.insert("handles".into(), Self::ref_to("handles", "collection"));
    map.insert("open".into(), Self::ref_to("meta/open", "action"));
    map.insert("stat".into(), Self::ref_to("meta/stat", "action"));
    map.insert("mkdir".into(), Self::ref_to("meta/mkdir", "action"));
    map.insert("rmdir".into(), Self::ref_to("meta/rmdir", "action"));
    map.insert("unlink".into(), Self::ref_to("meta/unlink", "action"));
    map.insert("rename".into(), Self::ref_to("meta/rename", "action"));
    map.insert("meta".into(), Self::ref_to("meta", "meta"));
    Value::Map(map)
}
```

### Handles Listing

```rust
fn read_handles_listing(&self) -> Value {
    let items: Vec<Value> = self.handles.keys()
        .map(|id| Self::ref_to(&format!("handles/{}", id), "handle"))
        .collect();

    let mut map = BTreeMap::new();
    map.insert("items".into(), Value::Array(items));
    Value::Map(map)
}
```

### Meta Handle Response

```rust
fn meta_handle(&self, id: u64, handle: &FileHandle) -> Value {
    let prefix = format!("meta/handles/{}", id);
    let data_prefix = format!("handles/{}", id);

    let mut state = BTreeMap::new();
    state.insert("position".into(), Value::Integer(handle.position as i64));
    state.insert("encoding".into(), Value::String(format!("{:?}", handle.encoding)));
    state.insert("mode".into(), Value::String(format!("{:?}", handle.mode)));
    state.insert("file".into(), Value::String(handle.path.clone()));

    let mut map = BTreeMap::new();
    map.insert("state".into(), Value::Map(state));
    map.insert("position".into(), Self::ref_to(&format!("{}/position", prefix), "integer"));
    map.insert("encoding".into(), Self::ref_to(&format!("{}/encoding", prefix), "string"));
    map.insert("close".into(), Self::ref_to(&format!("{}/close", prefix), "action"));
    map.insert("content".into(), Self::ref_to(&data_prefix, "stream"));
    map.insert("at".into(), Self::ref_to(&format!("{}/at", data_prefix), "accessor"));
    Value::Map(map)
}
```

## Migration

### Phase 1: Add Reference Responses

- Update `read_root()` to return references
- Update `read_handles_listing()` to return reference array
- Update `meta_handle()` to include navigation references
- Keep old behavior available for backwards compatibility if needed

### Phase 2: Update Meta Actions

- `meta/open`, `meta/stat`, etc. return action descriptors with `target`, `accepts`, `returns`

### Phase 3: Pagination Support

- Add `handles/after/{id}` path for cursor-based pagination
- Include `next` reference in handles listing when more items exist

## Test Changes

Update tests to expect reference structures:

```rust
#[test]
fn root_returns_references() {
    let mut store = FsStore::new();
    let record = store.read(&path!("")).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();

    if let Value::Map(map) = value {
        // Check handles is a reference
        if let Some(Value::Map(handles_ref)) = map.get("handles") {
            assert!(handles_ref.contains_key("path"));
            assert_eq!(
                handles_ref.get("path"),
                Some(&Value::String("handles".into()))
            );
        } else {
            panic!("Expected handles reference");
        }
    } else {
        panic!("Expected map");
    }
}

#[test]
fn handles_listing_returns_references() {
    // ... open some files ...

    let record = store.read(&path!("handles")).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();

    if let Value::Map(map) = value {
        if let Some(Value::Array(items)) = map.get("items") {
            for item in items {
                assert!(Reference::from_value(item).is_some());
            }
        } else {
            panic!("Expected items array");
        }
    } else {
        panic!("Expected map");
    }
}
```

## Summary

| Path | Before | After |
|------|--------|-------|
| `/` | Help strings | References to operations |
| `handles` | `[0, 1, 2]` | `{items: [{path: "handles/0"}, ...]}` |
| `handles/0` | Content | Content (unchanged) |
| `meta/handles/0` | Schema + state | State + reference navigation |
| `meta/open` | Schema | Action descriptor with target |

Clients discover the API by following references from the root. No path
construction, no external documentation required.
