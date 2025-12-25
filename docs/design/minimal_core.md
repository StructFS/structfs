# Design: A Minimal Core for StructFS

## The Problem

The current `store` crate is fighting Rust's type system. The `Reader` trait requires:

```rust
fn read_to_deserializer<'de, 'this>(
    &'this mut self,
    from: &Path,
) -> Result<Option<Box<dyn erased_serde::Deserializer<'de>>>, Error>
where
    'this: 'de;

fn read_owned<RecordType: DeserializeOwned>(
    &mut self,
    from: &Path,
) -> Result<Option<RecordType>, Error>;
```

This design exists to support:
1. Static type safety at call sites (`read_owned::<MyType>`)
2. Dynamic composition via `OverlayStore` (requires object safety)

The result: `erased_serde` in the public interface, callback-based patterns for object safety, and `ObjectSafeReader`/`ObjectSafeWriter`/`StoreWrapper` machinery that's hard to follow.

But look at how stores are actually used. The REPL does this:

```rust
// StoreContext
pub fn read(&mut self, path: &Path) -> Result<Option<JsonValue>, ContextError> {
    Ok(self.store.read_owned(path)?)
}

pub fn write(&mut self, path: &Path, value: &JsonValue) -> Result<Path, ContextError> {
    Ok(self.store.write(path, value)?)
}
```

Every store implementation internally works with `serde_json::Value`. The static typing at the edges is a convenience, not a necessity.

## The Insight

From the manifesto:

> **struct** - a tree-shaped data structure read from or written to a StructFS Store

A struct is a tree. JSON is one serialization of trees. The core doesn't need serde - it needs a tree representation.

## Proposal: The Minimal Core

### Core Types

```rust
// packages/store/src/value.rs

/// A tree-shaped value that can be read from or written to a Store.
///
/// This is the universal data representation in StructFS. It maps directly
/// to JSON, MessagePack, CBOR, etc., but is encoding-agnostic.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}
```

Why each variant:
- `Null`: absence of value, distinct from "path doesn't exist"
- `Bool`, `Integer`, `Float`, `String`: primitive types present in every serialization format
- `Bytes`: binary data (images, files, etc.) - something JSON elides but CBOR/MessagePack handle
- `Array`: ordered sequences
- `Map`: the "struct" part - key/value pairs with string keys

Note: using `BTreeMap` instead of `HashMap` for deterministic ordering.

### Core Traits

```rust
// packages/store/src/store.rs

pub trait Reader {
    fn read(&mut self, from: &Path) -> Result<Option<Value>, Error>;
}

pub trait Writer {
    fn write(&mut self, to: &Path, data: Value) -> Result<Path, Error>;
}

pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}
```

That's it. No generics. No lifetimes fighting each other. No `erased_serde`.

### Why This Works

**Object safety comes free.** Both traits are object-safe without any additional machinery:

```rust
// This just works
type BoxedStore = Box<dyn Store + Send + Sync>;
```

**Composition becomes trivial.** `OverlayStore` shrinks dramatically:

```rust
pub struct OverlayStore {
    layers: Vec<(Path, Box<dyn Store + Send + Sync>)>,
}

impl Reader for OverlayStore {
    fn read(&mut self, from: &Path) -> Result<Option<Value>, Error> {
        for (prefix, store) in self.layers.iter_mut().rev() {
            if from.has_prefix(prefix) {
                let suffix = from.strip_prefix(prefix).unwrap();
                return store.read(&suffix);
            }
        }
        Err(Error::NoRoute { path: from.clone() })
    }
}

impl Writer for OverlayStore {
    fn write(&mut self, to: &Path, data: Value) -> Result<Path, Error> {
        for (prefix, store) in self.layers.iter_mut().rev() {
            if to.has_prefix(prefix) {
                let suffix = to.strip_prefix(prefix).unwrap();
                return store.write(&suffix, data);
            }
        }
        Err(Error::NoRoute { path: to.clone() })
    }
}
```

No `ObjectSafeStore`, no `StoreWrapper`, no callbacks.

### Path Operations on Value

The `Value` type needs tree navigation to support sub-path reads:

```rust
impl Value {
    /// Get a reference to a nested value by path.
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

    /// Set a nested value by path, creating intermediate Maps as needed.
    pub fn set(&mut self, path: &Path, value: Value) -> Result<(), Error> {
        // ... implementation
    }

    /// Remove a value at path.
    pub fn remove(&mut self, path: &Path) -> Result<Option<Value>, Error> {
        // ... implementation
    }
}
```

This addresses the "no delete semantics" gap in the current design. The manifesto doesn't define delete, but stores need it internally.

### Serde Integration as a Layer

Serde becomes an optional adapter, not a core dependency:

```rust
// packages/store-serde/src/lib.rs (or packages/store/src/serde.rs behind a feature flag)

use serde::{Serialize, de::DeserializeOwned};
use structfs_store::{Value, Path, Error, Reader, Writer};

/// Convert a Rust type to a Value.
pub fn to_value<T: Serialize>(value: &T) -> Result<Value, Error> {
    // Implementation using serde
}

/// Convert a Value to a Rust type.
pub fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, Error> {
    // Implementation using serde
}

/// Extension trait for typed reads.
pub trait TypedReader: Reader {
    fn read_as<T: DeserializeOwned>(&mut self, from: &Path) -> Result<Option<T>, Error> {
        match self.read(from)? {
            Some(value) => Ok(Some(from_value(value)?)),
            None => Ok(None),
        }
    }
}

impl<R: Reader> TypedReader for R {}

/// Extension trait for typed writes.
pub trait TypedWriter: Writer {
    fn write_as<T: Serialize>(&mut self, to: &Path, data: &T) -> Result<Path, Error> {
        self.write(to, to_value(data)?)
    }
}

impl<W: Writer> TypedWriter for W {}
```

Now the REPL can do:

```rust
use structfs_store_serde::{TypedReader, TypedWriter};

// If you want dynamic JSON-like access:
let value: Value = store.read(&path)?.unwrap_or(Value::Null);

// If you want typed access:
let config: MyConfig = store.read_as(&path)?.ok_or(Error::NotFound)?;
```

The core crate doesn't depend on serde at all.

## Migration Path

### Phase 1: Add Value Type

Add `Value` alongside the existing traits. Implement `From<serde_json::Value>` and `Into<serde_json::Value>` for easy conversion.

### Phase 2: Add Simple Traits

Add `SimpleReader` and `SimpleWriter` (or similar) that use `Value`. Existing stores can implement both the old and new traits during transition.

### Phase 3: Update Stores

Migrate stores to implement the new traits. The in-memory store becomes trivial:

```rust
pub struct InMemoryStore {
    root: Value,
}

impl Reader for InMemoryStore {
    fn read(&mut self, from: &Path) -> Result<Option<Value>, Error> {
        Ok(self.root.get(from).cloned())
    }
}

impl Writer for InMemoryStore {
    fn write(&mut self, to: &Path, data: Value) -> Result<Path, Error> {
        self.root.set(to, data)?;
        Ok(to.clone())
    }
}
```

### Phase 4: Remove Old Traits

Once all stores are migrated, remove the serde-based traits from core.

## What Stays, What Goes

### Keep

- `Path` - already minimal and well-designed
- `Error` - the variants are reasonable
- The mount/overlay composition model
- The docs protocol pattern

### Remove from Core

- `erased_serde` dependency
- `ObjectSafeReader`, `ObjectSafeWriter`, `ObjectSafeStore`
- `ReaderWrapper`, `WriterWrapper`, `StoreWrapper`
- The callback-based `object_safe_read_to_deserializer` pattern
- `read_to_deserializer` method
- `Capability<AuthorityT>` (unused, can be added back when actually needed)
- `Reference` (design it properly when implementing lazy loading)
- `AsyncReader`, `AsyncWriter` (add back with the simple trait design when needed)

### Move Out of Core

- `MountConfig` - belongs in application layer, not core
- `LocalStoreError` - specific to local filesystem stores
- Serde integration - behind a feature flag or separate crate

## The Resulting Core

After cleanup, `packages/store/src/` contains:

```
lib.rs          - re-exports
path.rs         - Path type (mostly unchanged)
value.rs        - Value enum and tree operations
store.rs        - Reader, Writer traits
error.rs        - Error types
overlay.rs      - OverlayStore (dramatically simplified)
```

Total: ~500 lines of straightforward code, no lifetime gymnastics, no macro magic, no erased_serde.

## Open Questions

### 1. Bytes vs Base64 Strings

Should `Value::Bytes` exist? JSON doesn't have bytes, so any JSON-based store would need to encode them (typically base64). Options:

- **Include Bytes**: The core is format-agnostic. Stores that can't handle bytes convert to/from base64 at their boundary.
- **Exclude Bytes**: Keep the core aligned with JSON's data model. Binary data uses base64 strings everywhere.

Recommendation: Include Bytes. The core shouldn't be limited by JSON's quirks. CBOR and MessagePack handle bytes natively.

### 2. Integer Precision

`i64` handles most cases, but JavaScript (and thus JSON in many contexts) only safely represents integers up to 2^53. Options:

- **Use i64**: Accept the precision difference. Most data fits in 53 bits anyway.
- **Use BigInt**: Add a `BigInteger(String)` variant for arbitrary precision.
- **Use f64 only**: Match JSON semantics exactly.

Recommendation: Use i64. The manifesto says StructFS is format-agnostic. JavaScript's limitations shouldn't constrain the core.

### 3. Map Key Type

Currently `BTreeMap<String, Value>`. Should non-string keys be supported?

- **String only**: Matches JSON, simpler, path components are strings anyway.
- **Value keys**: More general, matches some formats (CBOR allows any key).

Recommendation: String only. Path components are strings. Map keys become path components. Keep it simple.

### 4. Delete Semantics

The manifesto doesn't define delete. Current implementation uses "write null to unmount" but that's specific to mounts. Options:

- **Write Null deletes**: Writing `Value::Null` to a path removes it from the parent.
- **Explicit remove**: Add `fn remove(&mut self, path: &Path) -> Result<Option<Value>, Error>` to Writer.
- **Convention per store**: Each store decides. Some may be append-only.

Recommendation: Add explicit `remove` to the trait. "Write null" can be a convention stores implement on top, but having a clear delete operation makes the model complete.

## Summary

The current store crate's complexity comes from forcing serde's compile-time type safety into a fundamentally dynamic system. By making `Value` the core abstraction:

1. Object safety comes free
2. Composition becomes trivial
3. The core shrinks by ~70%
4. Serde becomes an optional convenience layer
5. New serialization formats (CBOR, MessagePack) integrate easily

The manifesto's vision - "everything is a struct at a path" - maps directly to `Value` at `Path`. That's the minimal core.
