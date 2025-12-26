# Plan 3: Filesystem Position Abstraction

## Problem

File position is hidden in `std::fs::File` internal state. It cannot be queried, and operations have implicit position semantics. This breaks the "everything addressable by path" principle.

Current behavior:
- `read /sys/fs/handles/42` - reads from current position to EOF
- `write /sys/fs/handles/42/seek {"pos": 100}` - changes position (side effect)
- No way to query current position
- Position is invisible state

## Design Decision

**Position as addressable state.** Every file operation can specify position, and position is queryable.

## New Path Structure

```
/sys/fs/handles                         # List open handle IDs
/sys/fs/handles/{id}                    # Read from current position to EOF
/sys/fs/handles/{id}/at/{offset}        # Read from offset to EOF
/sys/fs/handles/{id}/at/{offset}/len/{n}  # Read n bytes from offset
/sys/fs/handles/{id}/position           # Current position (readable/writable)
/sys/fs/handles/{id}/meta               # File metadata
/sys/fs/handles/{id}/close              # Close handle (write null)
```

## Implementation Steps

### Step 1: Add explicit position tracking to FileHandle

**File:** `packages/sys/src/fs.rs`

```rust
struct FileHandle {
    file: File,
    path: String,
    mode: OpenMode,
    position: u64,  // Explicit position tracking (mirrors file.stream_position())
}
```

### Step 2: Define handle operation enum

```rust
enum HandleOperation {
    /// Read from current position to EOF
    ReadDefault,
    /// Read from offset to EOF
    ReadAt { offset: u64 },
    /// Read n bytes from offset
    ReadAtLen { offset: u64, length: u64 },
    /// Get/set position
    Position,
    /// Get file metadata
    Meta,
    /// Close handle
    Close,
    /// Write at current position
    WriteDefault,
    /// Write at offset
    WriteAt { offset: u64 },
    /// Invalid path
    Invalid,
}

fn parse_handle_operation(path: &Path) -> (Option<u64>, HandleOperation) {
    // /handles/{id} -> ReadDefault
    // /handles/{id}/at/{offset} -> ReadAt
    // /handles/{id}/at/{offset}/len/{length} -> ReadAtLen
    // /handles/{id}/position -> Position
    // /handles/{id}/meta -> Meta
    // /handles/{id}/close -> Close

    if path.len() < 2 || path[0] != "handles" {
        return (None, HandleOperation::Invalid);
    }

    let id: u64 = match path[1].parse() {
        Ok(id) => id,
        Err(_) => return (None, HandleOperation::Invalid),
    };

    match path.len() {
        2 => (Some(id), HandleOperation::ReadDefault),
        3 => match path[2].as_str() {
            "position" => (Some(id), HandleOperation::Position),
            "meta" => (Some(id), HandleOperation::Meta),
            "close" => (Some(id), HandleOperation::Close),
            _ => (Some(id), HandleOperation::Invalid),
        },
        4 if path[2] == "at" => {
            match path[3].parse() {
                Ok(offset) => (Some(id), HandleOperation::ReadAt { offset }),
                Err(_) => (Some(id), HandleOperation::Invalid),
            }
        },
        6 if path[2] == "at" && path[4] == "len" => {
            let offset: u64 = path[3].parse().ok()?;
            let length: u64 = path[5].parse().ok()?;
            (Some(id), HandleOperation::ReadAtLen { offset, length })
        },
        _ => (Some(id), HandleOperation::Invalid),
    }
}
```

### Step 3: Implement position-aware reads

```rust
impl Reader for FsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Handle /handles listing
        if from.len() == 1 && from[0] == "handles" {
            let ids: Vec<Value> = self.handles.keys()
                .map(|id| Value::Integer(*id as i64))
                .collect();
            return Ok(Some(Record::parsed(Value::Array(ids))));
        }

        let (id, op) = parse_handle_operation(from);
        let id = id.ok_or_else(|| Error::Other {
            message: "Invalid handle path".into()
        })?;

        let handle = self.handles.get_mut(&id)
            .ok_or_else(|| Error::Other {
                message: format!("Handle {} not found", id)
            })?;

        match op {
            HandleOperation::ReadDefault => {
                self.read_from_position(handle)
            }
            HandleOperation::ReadAt { offset } => {
                handle.file.seek(SeekFrom::Start(offset))?;
                handle.position = offset;
                self.read_from_position(handle)
            }
            HandleOperation::ReadAtLen { offset, length } => {
                handle.file.seek(SeekFrom::Start(offset))?;
                handle.position = offset;
                self.read_length(handle, length)
            }
            HandleOperation::Position => {
                let mut map = BTreeMap::new();
                map.insert("position".to_string(), Value::Integer(handle.position as i64));
                Ok(Some(Record::parsed(Value::Map(map))))
            }
            HandleOperation::Meta => {
                self.read_meta(handle)
            }
            _ => Err(Error::Other { message: "Invalid read operation".into() }),
        }
    }
}

fn read_from_position(&mut self, handle: &mut FileHandle) -> Result<Option<Record>, Error> {
    let mut buffer = Vec::new();
    handle.file.read_to_end(&mut buffer)?;
    handle.position = handle.file.stream_position()?;

    let encoded = base64::Engine::encode(&STANDARD, &buffer);
    Ok(Some(Record::parsed(Value::String(encoded))))
}

fn read_length(&mut self, handle: &mut FileHandle, length: u64) -> Result<Option<Record>, Error> {
    let mut buffer = vec![0u8; length as usize];
    handle.file.read_exact(&mut buffer)?;
    handle.position = handle.file.stream_position()?;

    let encoded = base64::Engine::encode(&STANDARD, &buffer);
    Ok(Some(Record::parsed(Value::String(encoded))))
}
```

### Step 4: Implement position write (seek)

```rust
impl Writer for FsStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        let (id, op) = parse_handle_operation(to);

        // Handle position write
        if let (Some(id), HandleOperation::Position) = (id, &op) {
            let value = data.into_value(&NoCodec)?;
            let pos = match value.get("pos") {
                Some(Value::Integer(p)) => *p as u64,
                _ => return Err(Error::Other {
                    message: "Expected {\"pos\": <integer>}".into()
                }),
            };

            let handle = self.handles.get_mut(&id)
                .ok_or_else(|| Error::Other {
                    message: format!("Handle {} not found", id)
                })?;

            handle.file.seek(SeekFrom::Start(pos))?;
            handle.position = pos;
            return Ok(to.clone());
        }

        // Handle write at offset
        if let (Some(id), HandleOperation::WriteAt { offset }) = (id, &op) {
            let handle = self.handles.get_mut(&id)
                .ok_or_else(|| Error::Other {
                    message: format!("Handle {} not found", id)
                })?;

            handle.file.seek(SeekFrom::Start(*offset))?;
            handle.position = *offset;
            return self.write_data(handle, data, to);
        }

        // ... rest of write handling
    }
}
```

### Step 5: Add handle listing

```rust
// read /sys/fs/handles -> [0, 1, 2, ...]
if from.len() == 1 && from[0] == "handles" {
    let ids: Vec<Value> = self.handles.keys()
        .map(|id| Value::Integer(*id as i64))
        .collect();
    return Ok(Some(Record::parsed(Value::Array(ids))));
}
```

## Path Summary

| Path | Read | Write |
|------|------|-------|
| `/sys/fs/open` | - | Open file, returns handle path |
| `/sys/fs/handles` | List open handle IDs | - |
| `/sys/fs/handles/{id}` | Read from current pos to EOF | Write at current pos |
| `/sys/fs/handles/{id}/at/{offset}` | Read from offset to EOF | Write at offset |
| `/sys/fs/handles/{id}/at/{offset}/len/{n}` | Read n bytes from offset | - |
| `/sys/fs/handles/{id}/position` | Get `{"position": n}` | Set `{"pos": n}` |
| `/sys/fs/handles/{id}/meta` | File metadata | - |
| `/sys/fs/handles/{id}/close` | - | Close (write null) |

## Usage Examples

```bash
# Open a file
@handle write /ctx/sys/fs/open {"path": "/tmp/test.txt", "mode": "read_write"}
# Returns: /ctx/sys/fs/handles/0

# Read entire file
read /ctx/sys/fs/handles/0

# Read from specific position
read /ctx/sys/fs/handles/0/at/100

# Read 50 bytes from position 100
read /ctx/sys/fs/handles/0/at/100/len/50

# Check current position
read /ctx/sys/fs/handles/0/position
# Returns: {"position": 150}

# Seek to position
write /ctx/sys/fs/handles/0/position {"pos": 0}

# Write at specific offset
write /ctx/sys/fs/handles/0/at/200 "data at 200"

# Close
write /ctx/sys/fs/handles/0/close null
```

## Tests

```rust
#[test]
fn test_position_query() {
    let mut store = FsStore::new();
    let handle = open_test_file(&mut store, "r");

    let pos = store.read(&handle.join(&path!("position"))).unwrap().unwrap();
    let value = pos.into_value(&NoCodec).unwrap();

    assert_eq!(value.get("position"), Some(&Value::Integer(0)));
}

#[test]
fn test_read_at_offset() {
    let mut store = FsStore::new();
    let handle = open_test_file(&mut store, "r");  // File contains "hello world"

    // Read from position 6
    let content = store.read(&handle.join(&path!("at/6"))).unwrap().unwrap();
    // Should return "world" (base64 encoded)
}

#[test]
fn test_position_persists_after_read() {
    let mut store = FsStore::new();
    let handle = open_test_file(&mut store, "r");

    // Read 5 bytes from position 0
    store.read(&handle.join(&path!("at/0/len/5"))).unwrap();

    // Position should now be 5
    let pos = store.read(&handle.join(&path!("position"))).unwrap().unwrap();
    let value = pos.into_value(&NoCodec).unwrap();
    assert_eq!(value.get("position"), Some(&Value::Integer(5)));
}

#[test]
fn test_write_at_offset() {
    let mut store = FsStore::new();
    let handle = open_test_file(&mut store, "rw");

    // Write at position 10
    store.write(
        &handle.join(&path!("at/10")),
        Record::parsed(Value::String("inserted".into()))
    ).unwrap();

    // Position should now be 18 (10 + len("inserted"))
    let pos = store.read(&handle.join(&path!("position"))).unwrap().unwrap();
    let value = pos.into_value(&NoCodec).unwrap();
    assert_eq!(value.get("position"), Some(&Value::Integer(18)));
}
```

## Files Changed

- `packages/sys/src/fs.rs` - Major refactor for position-aware operations

## Complexity

Medium-High - Significant changes to path parsing and operation semantics, but contained to fs.rs.
