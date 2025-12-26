# Plan 5: Error Type Cleanup

## Problem

Too many generic error variants with unclear usage:

```rust
pub enum Error {
    Path(PathError),
    InvalidPath { message: String },  // When vs Path?
    NoRoute { path: Path },
    Decode { format: Format, message: String },
    Encode { format: Format, message: String },
    UnsupportedFormat(Format),
    Ll(structfs_ll_store::LLError),
    Other { message: String },  // Catch-all
}
```

Issues:
- `InvalidPath` vs `Path(PathError)` - which to use?
- `Other` is a catch-all that loses context
- No I/O error variant (stores wrap `std::io::Error` in `Other`)
- No store identification in errors

## Proposed Error Structure

```rust
/// Errors that can occur during store operations
#[derive(Debug, Error)]
pub enum Error {
    /// Path parsing or validation failed
    #[error("path error: {0}")]
    Path(#[from] PathError),

    /// No store mounted at the given path
    #[error("no route to {path}")]
    NoRoute { path: Path },

    /// Codec error during encode/decode
    #[error("{operation} failed for format {format}: {message}")]
    Codec {
        operation: CodecOperation,
        format: Format,
        message: String,
    },

    /// Format not supported by this store
    #[error("unsupported format: {0}")]
    UnsupportedFormat(Format),

    /// Low-level store error
    #[error("low-level error: {0}")]
    Ll(#[from] structfs_ll_store::LLError),

    /// I/O error (filesystem, network)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Store-specific error with context
    #[error("{store}::{operation}: {message}")]
    Store {
        store: &'static str,
        operation: &'static str,
        message: String,
    },
}

/// Whether a codec error occurred during encoding or decoding
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecOperation {
    Encode,
    Decode,
}

impl std::fmt::Display for CodecOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecOperation::Encode => write!(f, "encode"),
            CodecOperation::Decode => write!(f, "decode"),
        }
    }
}
```

## Changes Summary

| Old | New | Rationale |
|-----|-----|-----------|
| `InvalidPath { message }` | `Path(PathError)` | Use the structured error |
| `Decode { format, message }` | `Codec { operation: Decode, ... }` | Merge with Encode |
| `Encode { format, message }` | `Codec { operation: Encode, ... }` | Merge with Decode |
| `Other { message }` | `Store { store, operation, message }` | Add context |
| (none) | `Io(std::io::Error)` | First-class I/O errors |

## Implementation Steps

### Step 1: Update error enum

**File:** `packages/core-store/src/error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("path error: {0}")]
    Path(#[from] PathError),

    #[error("no route to {path}")]
    NoRoute { path: Path },

    #[error("{operation} failed for format {format}: {message}")]
    Codec {
        operation: CodecOperation,
        format: Format,
        message: String,
    },

    #[error("unsupported format: {0}")]
    UnsupportedFormat(Format),

    #[error("low-level error: {0}")]
    Ll(#[from] structfs_ll_store::LLError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{store}::{operation}: {message}")]
    Store {
        store: &'static str,
        operation: &'static str,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecOperation {
    Encode,
    Decode,
}
```

### Step 2: Add helper constructors

```rust
impl Error {
    /// Create a store-specific error
    pub fn store(store: &'static str, operation: &'static str, message: impl Into<String>) -> Self {
        Error::Store {
            store,
            operation,
            message: message.into(),
        }
    }

    /// Create a codec decode error
    pub fn decode(format: Format, message: impl Into<String>) -> Self {
        Error::Codec {
            operation: CodecOperation::Decode,
            format,
            message: message.into(),
        }
    }

    /// Create a codec encode error
    pub fn encode(format: Format, message: impl Into<String>) -> Self {
        Error::Codec {
            operation: CodecOperation::Encode,
            format,
            message: message.into(),
        }
    }
}
```

### Step 3: Migrate existing error usage

**Before:**
```rust
// In http broker
Err(Error::Other {
    message: format!("Request {} not found", id),
})

// In fs store
Err(Error::Other {
    message: format!("Handle {} not found", id),
})

// In codec
Err(Error::Decode {
    format: Format::JSON,
    message: e.to_string(),
})
```

**After:**
```rust
// In http broker
Err(Error::store("http_broker", "read", format!("Request {} not found", id)))

// In fs store
Err(Error::store("fs", "read", format!("Handle {} not found", id)))

// In codec
Err(Error::decode(Format::JSON, e.to_string()))
```

### Step 4: Update stores to use new errors

**HTTP Broker:**
```rust
impl Reader for HttpBrokerStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let handle = self.handles.get_mut(&request_id)
            .ok_or_else(|| Error::store("http_broker", "read",
                format!("Request {} not found", request_id)))?;
        // ...
    }
}
```

**Filesystem:**
```rust
impl Reader for FsStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let handle = self.handles.get_mut(&id)
            .ok_or_else(|| Error::store("fs", "read",
                format!("Handle {} not found", id)))?;

        // I/O errors now use the Io variant automatically via From
        let mut buffer = Vec::new();
        handle.file.read_to_end(&mut buffer)?;  // ? converts io::Error -> Error::Io
        // ...
    }
}
```

**Mount Store:**
```rust
impl<F: StoreFactory> Reader for MountStore<F> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // NoRoute stays the same
        self.overlay.read(from).ok_or_else(|| Error::NoRoute {
            path: from.clone(),
        })
    }
}
```

### Step 5: Remove deprecated variants

After migration, ensure no code uses:
- `Error::InvalidPath` (use `Error::Path`)
- `Error::Other` (use `Error::Store`)
- `Error::Encode` (use `Error::Codec`)
- `Error::Decode` (use `Error::Codec`)

## Error Usage Guidelines

Document when to use each variant:

| Variant | When to Use |
|---------|-------------|
| `Path` | Path parsing/validation failed |
| `NoRoute` | No store mounted at path |
| `Codec` | Serialization/deserialization failed |
| `UnsupportedFormat` | Store doesn't support requested format |
| `Ll` | Low-level byte store error |
| `Io` | Filesystem, network I/O error |
| `Store` | Store-specific logic error (not found, invalid state, etc.) |

## Tests

```rust
#[test]
fn test_error_display() {
    let e = Error::store("http_broker", "read", "Request 42 not found");
    assert_eq!(e.to_string(), "http_broker::read: Request 42 not found");

    let e = Error::decode(Format::JSON, "unexpected EOF");
    assert_eq!(e.to_string(), "decode failed for format json: unexpected EOF");

    let e = Error::NoRoute { path: path!("foo/bar") };
    assert_eq!(e.to_string(), "no route to foo/bar");
}

#[test]
fn test_io_error_conversion() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let err: Error = io_err.into();
    assert!(matches!(err, Error::Io(_)));
}
```

## Files Changed

- `packages/core-store/src/error.rs` - Refactor error types
- `packages/http/src/core.rs` - Update error construction
- `packages/sys/src/fs.rs` - Update error construction
- `packages/json_store/src/in_memory.rs` - Update error construction
- All other stores - Update error construction

## Complexity

Medium - Many files touched but changes are mechanical.
