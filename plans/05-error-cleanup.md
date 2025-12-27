# Plan 5: Error Type Cleanup - ✅ COMPLETED

## Status: DONE (2025-12-27)

The core error type has been cleaned up. `Error::Other` has been removed and replaced with structured `Error::Store`.

## What Was Implemented

### Core Error Enum (packages/core-store/src/error.rs)

The error enum now has structured variants only:

```rust
pub enum Error {
    Path(PathError),
    NoRoute { path: Path },
    Codec { operation: CodecOperation, format: Format, message: String },
    UnsupportedFormat(Format),
    Ll(structfs_ll_store::LLError),
    Io(std::io::Error),
    Store { store: &'static str, operation: &'static str, message: String },
}
```

**No `Error::Other` variant.**

### Helper Methods

```rust
impl Error {
    pub fn store(store: &'static str, operation: &'static str, message: impl Into<String>) -> Self
    pub fn decode(format: Format, message: impl Into<String>) -> Self
    pub fn encode(format: Format, message: impl Into<String>) -> Self
}
```

### Error Display

Structured errors display with context:
```
http_broker::read: Request 42 not found
fs::write: Handle 5 not found
decode failed for format json: unexpected token
```

### HTTP Crate's Own Error Type

The HTTP crate (`packages/http/src/error.rs`) has its own `Error` enum with an `Other` variant. This is intentional - it's for HTTP-specific errors. When converting to core errors:

```rust
impl From<Error> for CoreError {
    fn from(error: Error) -> Self {
        CoreError::store("http", "request", error.to_string())
    }
}
```

## Tests

Comprehensive tests in `packages/core-store/src/error.rs`:
- `store_error_display` - Verifies `http_broker::read: Request 42 not found` format
- `codec_decode_error_display` / `codec_encode_error_display`
- `io_error_conversion` - `From<std::io::Error>`
- Error source chain tests

All passing.

## Minor Cleanup Remaining

The doc example in `packages/serde-store/src/typed.rs:30` references the old `Error::Other` API. This is a doc example only (marked `rust,ignore`), not actual code.
