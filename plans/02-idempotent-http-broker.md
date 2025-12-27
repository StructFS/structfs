# Plan 2: Idempotent HTTP Broker - ✅ COMPLETED

## Status: DONE (2025-12-27)

This plan has been fully implemented. The HTTP broker now caches responses and is idempotent.

## What Was Implemented

### SyncRequestHandle with Caching

```rust
struct SyncRequestHandle {
    request: HttpRequest,
    response: Option<HttpResponse>,  // Cached after execution
    error: Option<String>,           // Cached if execution failed
}
```

### Idempotent Read

From `packages/http/src/core.rs:412-421`:

```rust
// Execute on first read if not yet executed (idempotent)
if !handle.is_executed() {
    match self.executor.execute(&handle.request) {
        Ok(response) => handle.response = Some(response),
        Err(e) => handle.error = Some(e),
    }
}
// Return cached response or error
```

### Explicit Deletion

Writing `null` to a handle deletes it:
```rust
write /ctx/http/outstanding/{id} null
```

### Path Structure

| Path | Operation | Result |
|------|-----------|--------|
| `write /ctx/http {}` | Queue request | Returns `/ctx/http/outstanding/{id}` |
| `read /ctx/http/outstanding` | List handles | Returns `[0, 1, 2, ...]` |
| `read /ctx/http/outstanding/{id}` | Execute & return response | Returns cached response |
| `read /ctx/http/outstanding/{id}/request` | View queued request | Returns `HttpRequest` |
| `read /ctx/http/outstanding/{id}/response/status` | Navigate into response | Returns specific field |
| `write /ctx/http/outstanding/{id} null` | Delete handle | Removes handle |

## Tests

Comprehensive tests in `packages/http/src/core.rs`:
- `test_broker_idempotent_read` - Verifies same result on multiple reads
- `test_broker_idempotent_read_error_cached` - Errors are also cached
- `test_broker_delete_handle` - Explicit deletion works
- `test_broker_list_outstanding` - Listing works

All passing.
