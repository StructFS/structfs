# Plan 4: Registers as a Mounted Store

## Problem

Registers use special `@name` syntax instead of being a mounted store. This breaks uniformity - registers aren't addressable by normal paths.

Current state:
- `@name` syntax is parsed specially in command layer
- `*@name` dereferencing is string substitution
- RegisterStore exists but isn't mounted
- Cannot access registers via `/ctx/registers/` path

## Design Decision

Mount `RegisterStore` at `/ctx/registers/`. Keep `@` syntax as **sugar** that expands to the mount path.

## New Path Structure

```
/ctx/registers                    # List all register names
/ctx/registers/{name}             # Read/write register value
/ctx/registers/{name}/{path}      # Navigate into register value
```

## Syntax Sugar Transformation

The REPL transforms `@` syntax to mount paths:

| Sugar | Expands To |
|-------|------------|
| `@foo` | `/ctx/registers/foo` |
| `@foo/bar` | `/ctx/registers/foo/bar` |
| `*@foo` | Read `/ctx/registers/foo`, substitute value |

## Implementation Steps

### Step 1: Add MountConfig variant

**File:** `packages/core-store/src/mount_store.rs`

```rust
/// Configuration for creating a store
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MountConfig {
    /// In-memory JSON store
    Memory,
    /// HTTP client store
    Http { base_url: String },
    /// HTTP broker (sync)
    HttpBroker,
    /// HTTP broker (async)
    AsyncHttpBroker,
    /// System primitives
    Sys,
    /// Register storage (session-local named values)
    Registers,
}
```

### Step 2: Update factory to create RegisterStore

**File:** `packages/repl/src/store_context.rs`

```rust
impl StoreFactory for CoreReplStoreFactory {
    fn create(&self, config: &MountConfig) -> Result<StoreBox, CoreError> {
        match config {
            MountConfig::Memory => Ok(Box::new(InMemoryStore::new())),
            MountConfig::Http { base_url } => {
                Ok(Box::new(HttpClientStore::new(base_url.clone())))
            }
            MountConfig::HttpBroker => Ok(Box::new(HttpBrokerStore::new())),
            MountConfig::AsyncHttpBroker => Ok(Box::new(AsyncHttpBrokerStore::new())),
            MountConfig::Sys => Ok(Box::new(SysStore::new())),
            MountConfig::Registers => Ok(Box::new(RegisterStore::new())),
        }
    }
}
```

### Step 3: Mount registers in StoreContext initialization

**File:** `packages/repl/src/store_context.rs`

```rust
impl StoreContext {
    pub fn new() -> Result<Self, ContextError> {
        let mut store = MountStore::new(CoreReplStoreFactory);

        // Mount built-in stores (including registers)
        store.mount_store(path!("ctx/http"), Box::new(AsyncHttpBrokerStore::new()));
        store.mount_store(path!("ctx/http_sync"), Box::new(HttpBrokerStore::new()));
        store.mount_store(path!("ctx/sys"), Box::new(SysStore::new()));
        store.mount_store(path!("ctx/help"), Box::new(HelpStore::new()));
        store.mount_store(path!("ctx/registers"), Box::new(RegisterStore::new()));

        Ok(Self {
            store,
            current_path: Path::root(),
        })
    }
}
```

### Step 4: Remove embedded RegisterStore from StoreContext

**Before:**
```rust
pub struct StoreContext<F: StoreFactory = CoreReplStoreFactory> {
    store: MountStore<F>,
    registers: RegisterStore,  // REMOVE THIS
    current_path: Path,
}
```

**After:**
```rust
pub struct StoreContext<F: StoreFactory = CoreReplStoreFactory> {
    store: MountStore<F>,
    current_path: Path,
}
```

### Step 5: Update register access methods to use mount

```rust
impl<F: StoreFactory> StoreContext<F> {
    /// Read from a register (via mounted store)
    pub fn read_register(&mut self, name: &str) -> Result<Option<Value>, ContextError> {
        let path = Path::parse(&format!("ctx/registers/{}", name))
            .map_err(|e| ContextError::Path(e.to_string()))?;

        match self.store.read(&path) {
            Ok(Some(record)) => {
                let value = record.into_value(&NoCodec)
                    .map_err(|e| ContextError::Store(e.to_string()))?;
                Ok(Some(value))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ContextError::Store(e.to_string())),
        }
    }

    /// Write to a register (via mounted store)
    pub fn write_register(&mut self, name: &str, value: Value) -> Result<(), ContextError> {
        let path = Path::parse(&format!("ctx/registers/{}", name))
            .map_err(|e| ContextError::Path(e.to_string()))?;

        self.store.write(&path, Record::parsed(value))
            .map_err(|e| ContextError::Store(e.to_string()))?;
        Ok(())
    }

    /// List all register names (via mounted store)
    pub fn list_registers(&mut self) -> Result<Vec<String>, ContextError> {
        let path = path!("ctx/registers");

        match self.store.read(&path) {
            Ok(Some(record)) => {
                let value = record.into_value(&NoCodec)
                    .map_err(|e| ContextError::Store(e.to_string()))?;
                match value {
                    Value::Array(arr) => {
                        Ok(arr.into_iter().filter_map(|v| {
                            if let Value::String(s) = v { Some(s) } else { None }
                        }).collect())
                    }
                    _ => Ok(vec![]),
                }
            }
            Ok(None) => Ok(vec![]),
            Err(e) => Err(ContextError::Store(e.to_string())),
        }
    }
}
```

### Step 6: Update command parsing for `@` syntax sugar

**File:** `packages/repl/src/commands.rs`

```rust
/// Expand @syntax to /ctx/registers/ paths in user input
fn expand_register_syntax(input: &str) -> String {
    // Simple regex replacement: @name -> /ctx/registers/name
    // Handles: @foo, @foo/bar, but not *@foo (that's dereference)

    let mut result = String::new();
    let mut chars = input.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if c == '"' {
            in_string = !in_string;
            result.push(c);
        } else if c == '@' && !in_string {
            // Check if this is a dereference (*@)
            if result.ends_with('*') {
                // Don't expand dereferences here
                result.push(c);
            } else {
                // Expand @name to /ctx/registers/name
                let mut name = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' || next == '/' {
                        name.push(chars.next().unwrap());
                    } else {
                        break;
                    }
                }
                if !name.is_empty() {
                    result.push_str("/ctx/registers/");
                    result.push_str(&name);
                } else {
                    result.push('@');
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}
```

### Step 7: Update dereference to read from mounted store

```rust
fn resolve_dereference(path_str: &str, ctx: &mut StoreContext) -> Result<String, String> {
    if !path_str.contains("*@") {
        return Ok(path_str.to_string());
    }

    let mut result = String::new();
    let mut remaining = path_str;

    while let Some(pos) = remaining.find("*@") {
        result.push_str(&remaining[..pos]);

        let after = &remaining[pos + 2..];
        let name_end = after.find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(after.len());
        let register_name = &after[..name_end];

        if register_name.is_empty() {
            return Err("Empty register name after *@".to_string());
        }

        // Read from mounted store
        let value = ctx.read_register(register_name)
            .map_err(|e| format!("Failed to read register: {}", e))?
            .ok_or_else(|| format!("Register '{}' does not exist", register_name))?;

        let deref_value = match value {
            Value::String(s) => s,
            _ => return Err(format!(
                "Register '{}' does not contain a string (got {:?})",
                register_name, value
            )),
        };

        result.push_str(&deref_value);
        remaining = &after[name_end..];
    }

    result.push_str(remaining);
    Ok(result)
}
```

### Step 8: Update capture execution

```rust
fn execute_with_capture(register_name: &str, input: &str, ctx: &mut StoreContext) -> CommandResult {
    let result = execute_command(input, ctx);

    match result {
        CommandResult::Ok { display, capture } => {
            let value = extract_capture_value(display.as_ref(), capture);

            // Write to mounted register store
            if let Err(e) = ctx.write_register(register_name, value.clone()) {
                return CommandResult::Error(format!("Failed to save to register: {}", e));
            }

            let msg = format!(
                "{}\n{}",
                display.unwrap_or_default(),
                Color::Green.paint(format!("-> @{}", register_name))
            );
            CommandResult::ok_display(msg)
        }
        other => other,
    }
}
```

## Usage Examples

```bash
# These are now equivalent:
@result read /ctx/sys/time/now
read /ctx/registers/result                    # After expansion

# Reading registers - both work:
read @result                                  # Sugar
read /ctx/registers/result                    # Direct path

# Listing registers:
read /ctx/registers                           # Returns ["result", "handle", ...]
registers                                     # Command (still works)

# Nested access:
read @result/field                            # Sugar for /ctx/registers/result/field
read /ctx/registers/result/field              # Direct

# Dereferencing (unchanged syntax, different implementation):
@path read /ctx/sys/env/HOME
read *@path                                   # Reads register, uses value as path

# Deleting a register:
write @result null                            # Sugar
write /ctx/registers/result null              # Direct

# Copy register to register:
write @copy @original                         # Copies value
```

## Migration Notes

- All existing `@name` usage continues to work (syntax sugar)
- New capability: direct access via `/ctx/registers/` path
- RegisterStore is now a first-class mounted store
- Can be unmounted/remounted if needed (though not recommended)

## Tests

```rust
#[test]
fn test_register_via_mount_path() {
    let mut ctx = StoreContext::new().unwrap();

    // Write via direct path
    ctx.store.write(
        &path!("ctx/registers/test"),
        Record::parsed(Value::Integer(42))
    ).unwrap();

    // Read via direct path
    let result = ctx.store.read(&path!("ctx/registers/test")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();
    assert_eq!(value, Value::Integer(42));
}

#[test]
fn test_syntax_sugar_expansion() {
    assert_eq!(
        expand_register_syntax("read @foo"),
        "read /ctx/registers/foo"
    );
    assert_eq!(
        expand_register_syntax("read @foo/bar"),
        "read /ctx/registers/foo/bar"
    );
    // Dereference not expanded
    assert_eq!(
        expand_register_syntax("read *@foo"),
        "read *@foo"
    );
}

#[test]
fn test_list_registers_via_path() {
    let mut ctx = StoreContext::new().unwrap();

    ctx.write_register("a", Value::Integer(1)).unwrap();
    ctx.write_register("b", Value::Integer(2)).unwrap();

    let result = ctx.store.read(&path!("ctx/registers")).unwrap().unwrap();
    let value = result.into_value(&NoCodec).unwrap();

    match value {
        Value::Array(names) => {
            assert!(names.contains(&Value::String("a".into())));
            assert!(names.contains(&Value::String("b".into())));
        }
        _ => panic!("Expected array"),
    }
}
```

## Files Changed

- `packages/core-store/src/mount_store.rs` - Add `MountConfig::Registers`
- `packages/repl/src/store_context.rs` - Mount RegisterStore, remove embedded field
- `packages/repl/src/commands.rs` - Update `@` expansion and dereference

## Complexity

Medium - Touches multiple files but changes are straightforward refactoring.
