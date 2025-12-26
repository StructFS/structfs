# Plan 2: Idempotent HTTP Broker

## Problem

`HttpBrokerStore` uses `.remove()` on read, destroying state. Reading twice returns "not found" on second read. This violates REST idempotency and the "everything is a store" principle.

```rust
// Current destructive read (core.rs:94-99)
let request = self.requests.remove(&request_id)  // DESTROYS STATE
    .ok_or_else(|| ...)?;
```

## Design Principles

1. **Reads are idempotent** - Same path, same result
2. **State persists until explicitly deleted** - Write null to delete
3. **Lifecycle is explicit** - States: queued -> executed -> (deleted)
4. **Cleanup via store operations** - Not implicit consumption

## Solution

Change the broker to cache responses and support explicit deletion.

## Data Structure Change

```rust
/// A queued or completed HTTP request
struct RequestHandle {
    request: HttpRequest,
    response: Option<HttpResponse>,  // Cached after execution
    error: Option<String>,           // Cached if execution failed
}

pub struct HttpBrokerStore {
    handles: BTreeMap<u64, RequestHandle>,
    next_id: u64,
    executor: Box<dyn HttpExecutor>,
}
```

## Implementation Steps

### Step 1: Update data structures

**File:** `packages/http/src/core.rs`

```rust
struct RequestHandle {
    request: HttpRequest,
    response: Option<HttpResponse>,  // Cached after execution
    error: Option<String>,           // Cached if execution failed
}

pub struct HttpBrokerStore {
    handles: BTreeMap<u64, RequestHandle>,
    next_id: u64,
    executor: Box<dyn HttpExecutor>,
}
```

### Step 2: Update `read()` to be idempotent

```rust
fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
    // Handle /outstanding listing
    if from.len() == 1 && from[0] == "outstanding" {
        let ids: Vec<Value> = self.handles.keys()
            .map(|id| Value::Integer(*id as i64))
            .collect();
        return Ok(Some(Record::parsed(Value::Array(ids))));
    }

    // Parse /outstanding/{id} or /outstanding/{id}/request
    let (request_id, sub_path) = self.parse_request_path(from)?;

    let handle = self.handles.get_mut(&request_id)
        .ok_or_else(|| Error::Other {
            message: format!("Request {} not found", request_id),
        })?;

    // Return request details at /outstanding/{id}/request
    if sub_path.as_deref() == Some("request") {
        return Ok(Some(Record::parsed(request_to_value(&handle.request))));
    }

    // Execute on first read if not yet executed
    if handle.response.is_none() && handle.error.is_none() {
        match self.executor.execute(&handle.request) {
            Ok(response) => handle.response = Some(response),
            Err(e) => handle.error = Some(e.to_string()),
        }
    }

    // Return cached response or error
    if let Some(ref response) = handle.response {
        Ok(Some(Record::parsed(response_to_value(response))))
    } else if let Some(ref error) = handle.error {
        Err(Error::Other { message: error.clone() })
    } else {
        unreachable!()
    }
}
```

### Step 3: Add explicit deletion via write null

```rust
fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
    // Delete handle: write null to /outstanding/{id}
    if to.len() == 2 && to[0] == "outstanding" {
        let value = data.into_value(&NoCodec)?;
        if value == Value::Null {
            let id: u64 = to[1].parse().map_err(|_| Error::Other {
                message: "Invalid request ID".to_string(),
            })?;
            self.handles.remove(&id);
            return Ok(to.clone());
        }
    }

    // Queue new request: write to root
    if to.is_empty() {
        let request = parse_http_request(data)?;
        let id = self.next_id;
        self.next_id += 1;

        self.handles.insert(id, RequestHandle {
            request,
            response: None,
            error: None,
        });

        return Ok(Path::parse(&format!("outstanding/{}", id)).unwrap());
    }

    Err(Error::Other { message: "Invalid write path".to_string() })
}
```

### Step 4: Add listing support

Reading from `/outstanding` returns array of handle IDs:

```
read /ctx/http/outstanding  ->  [0, 1, 2]
```

## Path Structure

| Path | Operation | Result |
|------|-----------|--------|
| `write /ctx/http {}` | Queue request | Returns `/ctx/http/outstanding/{id}` |
| `read /ctx/http/outstanding` | List handles | Returns `[0, 1, 2, ...]` |
| `read /ctx/http/outstanding/{id}` | Execute & return response | Returns `HttpResponse` (cached) |
| `read /ctx/http/outstanding/{id}/request` | View queued request | Returns `HttpRequest` |
| `write /ctx/http/outstanding/{id} null` | Delete handle | Removes handle |

## Sync vs Async Broker Convergence

Both brokers should follow this pattern. The only difference:

- **Sync broker**: Blocks on first read until response arrives
- **Async broker**: Returns immediately with status, response available when complete

The async broker already caches responses - sync broker should match.

## Tests

```rust
#[test]
fn test_idempotent_read() {
    let mut broker = HttpBrokerStore::new(MockExecutor::new());

    // Queue request
    let handle = broker.write(&Path::root(), request_record()).unwrap();

    // First read - executes and caches
    let r1 = broker.read(&handle).unwrap().unwrap();

    // Second read - returns cached (idempotent)
    let r2 = broker.read(&handle).unwrap().unwrap();

    // Results should be identical
    assert_eq!(r1, r2);
}

#[test]
fn test_explicit_deletion() {
    let mut broker = HttpBrokerStore::new(MockExecutor::new());

    let handle = broker.write(&Path::root(), request_record()).unwrap();
    broker.read(&handle).unwrap();  // Execute

    // Delete
    broker.write(&handle, Record::parsed(Value::Null)).unwrap();

    // Now returns None or error
    let result = broker.read(&handle);
    assert!(result.is_err() || result.unwrap().is_none());
}

#[test]
fn test_list_handles() {
    let mut broker = HttpBrokerStore::new(MockExecutor::new());

    broker.write(&Path::root(), request_record()).unwrap();
    broker.write(&Path::root(), request_record()).unwrap();

    let list = broker.read(&path!("outstanding")).unwrap().unwrap();
    let value = list.into_value(&NoCodec).unwrap();

    match value {
        Value::Array(ids) => assert_eq!(ids.len(), 2),
        _ => panic!("Expected array"),
    }
}

#[test]
fn test_view_queued_request() {
    let mut broker = HttpBrokerStore::new(MockExecutor::new());

    let handle = broker.write(&Path::root(), request_record()).unwrap();

    // Read the queued request (not the response)
    let request_path = handle.join(&path!("request"));
    let result = broker.read(&request_path).unwrap().unwrap();

    // Should return the original request, not execute it
    let value = result.into_value(&NoCodec).unwrap();
    assert!(matches!(value, Value::Map(_)));
}
```

## Files Changed

- `packages/http/src/core.rs` - Refactor `HttpBrokerStore`
- `packages/http/src/types.rs` - Add `RequestHandle` struct if needed
- Add tests for idempotency, listing, deletion

## Complexity

Medium - Refactors core broker logic, but well-contained to HTTP package.
