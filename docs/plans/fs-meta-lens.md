# Implementation Plan: FsStore Meta Lens

This plan describes adding meta lens support to `packages/sys/src/fs.rs`.

## Overview

Add a `meta/` prefix to FsStore that exposes path affordances and enables
direct manipulation of handle state (e.g., seek via position write).

## Implementation Steps

### Step 1: Add Meta Path Detection

Modify `Reader::read` to detect `meta/` prefix and route to meta handling.

```rust
impl Reader for FsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // NEW: Check for meta prefix
        if from.len() > 0 && from[0] == "meta" {
            let rest = from.slice(1..);
            return self.read_meta(&rest);
        }

        // ... existing implementation ...
    }
}
```

### Step 2: Implement `read_meta` Method

Add a new method to handle meta reads:

```rust
impl FsStore {
    fn read_meta(&self, path: &Path) -> Result<Option<Record>, Error> {
        if path.is_empty() {
            return Ok(Some(Record::parsed(self.meta_root())));
        }

        match path[0].as_str() {
            "open" => Ok(Some(Record::parsed(self.meta_open()))),
            "handles" => self.read_meta_handles(&path.slice(1..)),
            "stat" => Ok(Some(Record::parsed(self.meta_stat()))),
            "mkdir" => Ok(Some(Record::parsed(self.meta_mkdir()))),
            "rmdir" => Ok(Some(Record::parsed(self.meta_rmdir()))),
            "unlink" => Ok(Some(Record::parsed(self.meta_unlink()))),
            "rename" => Ok(Some(Record::parsed(self.meta_rename()))),
            _ => Ok(None),
        }
    }
}
```

### Step 3: Implement Meta Response Builders

Add helper methods for each meta response:

```rust
fn meta_root(&self) -> Value {
    let mut fields = BTreeMap::new();

    fields.insert("open".to_string(), Value::Map({
        let mut m = BTreeMap::new();
        m.insert("writable".to_string(), Value::Bool(true));
        m.insert("description".to_string(),
                 Value::String("Open a file handle".into()));
        m
    }));

    fields.insert("handles".to_string(), Value::Map({
        let mut m = BTreeMap::new();
        m.insert("readable".to_string(), Value::Bool(true));
        m.insert("description".to_string(),
                 Value::String("Open file handles".into()));
        m
    }));

    // ... other fields ...

    let mut root = BTreeMap::new();
    root.insert("readable".to_string(), Value::Bool(true));
    root.insert("writable".to_string(), Value::Bool(true));
    root.insert("description".to_string(),
                Value::String("Filesystem operations".into()));
    root.insert("fields".to_string(), Value::Map(fields));

    Value::Map(root)
}
```

### Step 4: Implement Handle Meta

The key path: `meta/handles/{id}` and its sub-paths.

```rust
fn read_meta_handles(&self, path: &Path) -> Result<Option<Record>, Error> {
    if path.is_empty() {
        // List handles with basic info
        let ids: Vec<Value> = self.handles.keys()
            .map(|id| Value::Integer(*id as i64))
            .collect();

        let mut m = BTreeMap::new();
        m.insert("type".to_string(), Value::String("collection".into()));
        m.insert("description".to_string(),
                 Value::String("Open file handles".into()));
        m.insert("items".to_string(), Value::Array(ids));

        return Ok(Some(Record::parsed(Value::Map(m))));
    }

    // Parse handle ID
    let id: u64 = path[0].parse()
        .map_err(|_| Error::store("fs", "meta", "Invalid handle ID"))?;

    let handle = self.handles.get(&id)
        .ok_or_else(|| Error::store("fs", "meta",
                                     format!("Handle {} not found", id)))?;

    if path.len() == 1 {
        return Ok(Some(Record::parsed(self.meta_handle(handle))));
    }

    // Sub-path meta
    match path[1].as_str() {
        "position" => Ok(Some(Record::parsed(self.meta_position(handle)))),
        "meta" => Ok(Some(Record::parsed(self.meta_handle_meta()))),
        "at" => Ok(Some(Record::parsed(self.meta_at()))),
        "close" => Ok(Some(Record::parsed(self.meta_close()))),
        _ => Ok(None),
    }
}

fn meta_handle(&self, handle: &FileHandle) -> Value {
    let mut state = BTreeMap::new();
    state.insert("position".to_string(),
                 Value::Integer(handle.position as i64));
    state.insert("encoding".to_string(),
                 Value::String(format!("{:?}", handle.encoding)));
    state.insert("mode".to_string(),
                 Value::String(format!("{:?}", handle.mode)));
    state.insert("path".to_string(),
                 Value::String(handle.path.clone()));

    let mut fields = BTreeMap::new();
    // ... populate fields schema ...

    let mut m = BTreeMap::new();
    m.insert("readable".to_string(), Value::Bool(true));
    m.insert("writable".to_string(), Value::Bool(true));
    m.insert("state".to_string(), Value::Map(state));
    m.insert("fields".to_string(), Value::Map(fields));

    Value::Map(m)
}

fn meta_position(&self, handle: &FileHandle) -> Value {
    let mut m = BTreeMap::new();
    m.insert("readable".to_string(), Value::Bool(true));
    m.insert("writable".to_string(), Value::Bool(true));
    m.insert("type".to_string(), Value::String("integer".into()));
    m.insert("value".to_string(), Value::Integer(handle.position as i64));
    m.insert("description".to_string(),
             Value::String("Current byte offset. Write to seek.".into()));
    Value::Map(m)
}
```

### Step 5: Add Meta Writes

Modify `Writer::write` to handle meta prefix:

```rust
impl Writer for FsStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // NEW: Check for meta prefix
        if to.len() > 0 && to[0] == "meta" {
            let rest = to.slice(1..);
            return self.write_meta(&rest, data);
        }

        // ... existing implementation ...
    }
}
```

### Step 6: Implement Meta Write Handler

```rust
fn write_meta(&mut self, path: &Path, data: Record) -> Result<Path, Error> {
    // Only handles/*/position is writable via meta
    if path.len() < 3 || path[0] != "handles" {
        return Err(Error::store("fs", "meta",
                                format!("Cannot write to meta/{}", path)));
    }

    let id: u64 = path[1].parse()
        .map_err(|_| Error::store("fs", "meta", "Invalid handle ID"))?;

    match path[2].as_str() {
        "position" => {
            let value = data.into_value(&NoCodec)?;
            let pos = match value {
                Value::Integer(n) => n as u64,
                _ => return Err(Error::store("fs", "meta",
                                             "position must be integer")),
            };

            let handle = self.handles.get_mut(&id)
                .ok_or_else(|| Error::store("fs", "meta",
                                            format!("Handle {} not found", id)))?;

            handle.file.seek(SeekFrom::Start(pos))?;
            handle.position = pos;

            // Return the meta path we wrote to
            Ok(Path::parse(&format!("meta/handles/{}/position", id)).unwrap())
        }
        _ => Err(Error::store("fs", "meta",
                              format!("Cannot write to meta/handles/{}/{}",
                                      id, path[2]))),
    }
}
```

### Step 7: Add Tests

Test coverage for:

1. `read meta/` returns store schema
2. `read meta/open` returns open operation schema
3. `read meta/handles` lists handle IDs
4. `read meta/handles/{id}` returns handle affordances with live state
5. `read meta/handles/{id}/position` returns position meta
6. `write meta/handles/{id}/position 100` seeks the file
7. Error cases: invalid ID, nonexistent handle, unwritable paths

Example test:

```rust
#[test]
fn meta_handle_position_write_seeks() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");
    std::fs::write(&file_path, "0123456789").unwrap();

    let mut store = FsStore::new();

    // Open file
    let mut open_map = BTreeMap::new();
    open_map.insert("path".into(), Value::String(file_path.to_string_lossy().into()));
    open_map.insert("mode".into(), Value::String("read".into()));

    let handle_path = store
        .write(&path!("open"), Record::parsed(Value::Map(open_map)))
        .unwrap();

    let id = &handle_path[1]; // "0" or similar

    // Write to meta position
    let meta_pos_path = Path::parse(&format!("meta/handles/{}/position", id)).unwrap();
    store
        .write(&meta_pos_path, Record::parsed(Value::Integer(5)))
        .unwrap();

    // Verify position changed
    let pos_path = handle_path.join(&path!("position"));
    let record = store.read(&pos_path).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();

    if let Value::Map(m) = value {
        assert_eq!(m.get("position"), Some(&Value::Integer(5)));
    } else {
        panic!("Expected map");
    }
}
```

## File Changes Summary

| File | Changes |
|------|---------|
| `packages/sys/src/fs.rs` | Add meta prefix handling in read/write, add meta builder methods |

## Dependencies

None. This is self-contained within FsStore.

## Testing Strategy

1. Unit tests for each meta path (read and write where applicable)
2. Integration test: open file, read meta, write position via meta, verify seek
3. Error case coverage for invalid paths and nonexistent handles

## Future Considerations

- **MetaOverlay**: A wrapper that could add meta support to any store
- **Schema types**: Define proper schema types instead of ad-hoc Value maps
- **OpenAPI generation**: Use meta responses to generate API documentation
