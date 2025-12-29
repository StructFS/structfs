# Plan: HATEOAS Refactor for HTTP Stores

This plan describes refactoring all HTTP stores (HttpBrokerStore, AsyncHttpBrokerStore,
HttpClientStore) to follow hypermedia principles using the reference pattern. Clients
discover state transitions by following references in responses, not by constructing
paths from out-of-band knowledge.

## Current State: The Problems

### 1. Bare IDs Instead of References

```rust
// Current: read /outstanding -> [0, 1, 2]
let ids: Vec<Value> = self.handles.keys()
    .map(|id| Value::Integer(*id as i64))
    .collect();
```

Clients must know to construct `outstanding/{id}` from bare integers.

### 2. String Paths Instead of References

```rust
pub struct RequestStatus {
    pub response_path: Option<String>,  // Should be Reference
}
```

The `response_path` field is a plain string. Clients can't distinguish it from
arbitrary string data.

### 3. Root Returns Nothing Useful

Writing to root queues a request, but reading root returns nothing. There's no
entry point for clients to discover the API.

## Design Principles

### Separation of Data and Control

Like FsStore:
- **Data paths** (`outstanding/{id}/response`): Access request/response content
- **Control paths** (`meta/outstanding/{id}`): Inspect state, delete handles

### Docs vs Meta

**Docs**: Human-readable help text for the help system. Prose descriptions.

**Meta**: Machine-navigable structure. References, type information, action
descriptors.

Both are valuable. Docs explains "what does this do?" in prose. Meta enables
programmatic navigation without external knowledge.

### Broker Pattern Tension

The broker pattern has inherent tension with REST:
- Write queues request (no side effect on external system)
- Read executes request (side effect on first read)

This is by design—it enables async HTTP in a sync interface. The HATEOAS refactor
makes this explicit through action descriptors.

## Target Interface

### Root Returns References

**Before:** Root returns nothing on read.

**After:**
```json
{
  "outstanding": {"path": "outstanding", "type": {"name": "collection"}},
  "queue": {"path": "meta/queue", "type": {"name": "action"}},
  "meta": {"path": "meta", "type": {"name": "meta"}},
  "docs": {"path": "docs", "type": {"name": "docs"}}
}
```

Every field is a reference. To learn how to queue a request, follow `queue`.
For human-readable docs, follow `docs`.

### Outstanding Collection Returns References

**Before:** `[0, 1, 2]`

**After:**
```json
{
  "items": [
    {"path": "outstanding/0", "type": {"name": "request-handle"}},
    {"path": "outstanding/1", "type": {"name": "request-handle"}},
    {"path": "outstanding/2", "type": {"name": "request-handle"}}
  ]
}
```

Clients iterate `items` and follow references. No path construction from IDs.

### Handle Status Uses References

**Before (AsyncHttpBrokerStore):**
```json
{
  "id": "0",
  "state": "complete",
  "response_path": "outstanding/0/response"
}
```

**After:**
```json
{
  "state": "complete",
  "request": {"path": "outstanding/0/request", "type": {"name": "http-request"}},
  "response": {"path": "outstanding/0/response", "type": {"name": "http-response"}},
  "delete": {"path": "meta/outstanding/0/delete", "type": {"name": "action"}}
}
```

The `response` field is a reference. Clients can check `state`, then follow
`response` when ready.

### Meta Action Descriptors

```
read meta/queue
```

Returns:
```json
{
  "type": {"name": "action"},
  "method": "write",
  "target": {"path": ""},
  "accepts": {
    "method": {"type": {"name": "string"}, "required": true, "values": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]},
    "path": {"type": {"name": "string"}, "required": true},
    "headers": {"type": {"name": "map"}},
    "body": {"type": {"name": "string"}}
  },
  "returns": {
    "type": {"name": "request-handle"},
    "collection": {"path": "outstanding"}
  }
}
```

Clients discover:
- **method**: How to invoke (write)
- **target**: Where to write (root)
- **accepts**: What fields the request body supports
- **returns**: What they'll get back (handle in outstanding collection)

### Meta Handle (Async Broker)

```
read meta/outstanding/0
```

Returns:
```json
{
  "state": {
    "status": "pending",
    "method": "GET",
    "url": "https://httpbin.org/json"
  },
  "request": {"path": "outstanding/0/request", "type": {"name": "http-request"}},
  "response": {"path": "outstanding/0/response", "type": {"name": "http-response"}},
  "wait": {"path": "outstanding/0/response/wait", "type": {"name": "accessor"}},
  "delete": {"path": "meta/outstanding/0/delete", "type": {"name": "action"}}
}
```

The `state` field shows inline status. References tell clients how to navigate.

### Delete Action

```
read meta/outstanding/0/delete
```

Returns:
```json
{
  "type": {"name": "action"},
  "method": "write",
  "target": {"path": "outstanding/0"},
  "accepts": "null",
  "returns": "void"
}
```

Clients learn: write `null` to `outstanding/0` to delete.

### Docs Path (Unchanged)

The existing `docs` path continues to provide human-readable help:

```
read docs
```

Returns prose documentation for the help system. This complements meta—docs
explains in natural language, meta provides machine-navigable structure.

## Store-Specific Changes

### HttpBrokerStore (Sync)

| Path | Before | After |
|------|--------|-------|
| `read /` | Error | References to outstanding, meta/queue, meta, docs |
| `read /outstanding` | `[0, 1, 2]` | `{items: [refs...]}` |
| `read /outstanding/0` | Execute + response | Execute + response (unchanged) |
| `read /meta/queue` | N/A | Action descriptor |
| `read /meta/outstanding/0` | N/A | State + navigation references |
| `read /docs` | Prose help | Prose help (unchanged) |

### AsyncHttpBrokerStore

| Path | Before | After |
|------|--------|-------|
| `read /` | Error | References to outstanding, meta/queue, meta, docs |
| `read /outstanding` | `[0, 1, 2]` | `{items: [refs...]}` |
| `read /outstanding/0` | `RequestStatus` | `RequestStatus` with References |
| `read /meta/queue` | N/A | Action descriptor |
| `read /meta/outstanding/0` | N/A | State + navigation references |
| `read /meta/outstanding/0/delete` | N/A | Delete action descriptor |
| `read /docs` | Prose help | Prose help (unchanged) |

### HttpClientStore

The client store is different—it proxies to external URLs. HATEOAS applies to
store introspection, not proxied responses.

| Path | Before | After |
|------|--------|-------|
| `read /` | GET base_url | References to meta, docs, plus proxied content |
| `read /meta` | N/A | Store capabilities + action descriptors |
| `read /meta/get` | N/A | GET action descriptor |
| `read /meta/post` | N/A | POST action descriptor |
| `read /docs` | Prose help | Prose help (unchanged) |
| `read /{path}` | GET base_url/{path} | Unchanged (proxied) |

**Important**: External responses are not transformed. Only the store's own
structure uses references.

## RequestStatus Refactor

Update `RequestStatus` to use References:

```rust
pub struct RequestStatus {
    pub state: RequestState,
    pub error: Option<String>,
    pub request: Reference,    // Always present
    pub response: Option<Reference>,  // Present when complete
}

impl RequestStatus {
    pub fn pending(id: String) -> Self {
        Self {
            state: RequestState::Pending,
            error: None,
            request: Reference::with_type(
                format!("outstanding/{}/request", id),
                "http-request"
            ),
            response: None,
        }
    }

    pub fn complete(id: String) -> Self {
        Self {
            state: RequestState::Complete,
            error: None,
            request: Reference::with_type(
                format!("outstanding/{}/request", id),
                "http-request"
            ),
            response: Some(Reference::with_type(
                format!("outstanding/{}/response", id),
                "http-response"
            )),
        }
    }
}
```

The `id` field is removed—clients don't need it. They have the path.

## Implementation

### Reference Helper

Add dependency on `structfs_core_store::Reference`:

```rust
use structfs_core_store::Reference;

fn ref_to(path: &str, type_name: &str) -> Value {
    Reference::with_type(path, type_name).to_value()
}
```

### Root Response

```rust
fn read_root(&self) -> Value {
    let mut map = BTreeMap::new();
    map.insert("outstanding".into(), ref_to("outstanding", "collection"));
    map.insert("queue".into(), ref_to("meta/queue", "action"));
    map.insert("meta".into(), ref_to("meta", "meta"));
    map.insert("docs".into(), ref_to("docs", "docs"));
    Value::Map(map)
}
```

### Outstanding Listing

```rust
fn read_outstanding_listing(&self) -> Value {
    let items: Vec<Value> = self.handles.keys()
        .map(|id| ref_to(&format!("outstanding/{}", id), "request-handle"))
        .collect();

    let mut map = BTreeMap::new();
    map.insert("items".into(), Value::Array(items));
    Value::Map(map)
}
```

### Action Descriptor

```rust
fn queue_action_descriptor() -> Value {
    let mut accepts = BTreeMap::new();
    accepts.insert("method".into(), type_field("string", true, Some(vec!["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"])));
    accepts.insert("path".into(), type_field("string", true, None));
    accepts.insert("headers".into(), type_field("map", false, None));
    accepts.insert("body".into(), type_field("string", false, None));

    let mut returns = BTreeMap::new();
    returns.insert("type".into(), type_info("request-handle"));
    returns.insert("collection".into(), ref_to("outstanding", "collection"));

    let mut map = BTreeMap::new();
    map.insert("type".into(), type_info("action"));
    map.insert("method".into(), Value::String("write".into()));
    map.insert("target".into(), ref_to("", "root"));
    map.insert("accepts".into(), Value::Map(accepts));
    map.insert("returns".into(), Value::Map(returns));
    Value::Map(map)
}
```

## Migration

### Phase 1: Add Reference Support

1. Add `structfs_core_store::Reference` import to http crate
2. Update `RequestStatus` to use Reference for `response_path`
3. Add `read_root()` returning references (including docs)
4. Add `read_outstanding_listing()` returning reference array

### Phase 2: Add Meta Paths

1. Implement `meta/` prefix handling
2. Add `meta/queue` action descriptor
3. Add `meta/outstanding/{id}` state + navigation
4. Add `meta/outstanding/{id}/delete` action descriptor

### Phase 3: Update Tests

1. Update tests to expect reference structures
2. Verify docs path continues to work
3. Add tests for meta action descriptors

## Test Changes

```rust
#[test]
fn root_returns_references() {
    let store = HttpBrokerStore::with_default_timeout().unwrap();
    let record = store.read(&path!("")).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();

    if let Value::Map(map) = value {
        assert!(Reference::from_value(map.get("outstanding").unwrap()).is_some());
        assert!(Reference::from_value(map.get("queue").unwrap()).is_some());
        assert!(Reference::from_value(map.get("docs").unwrap()).is_some());
    } else {
        panic!("Expected map");
    }
}

#[test]
fn outstanding_listing_returns_references() {
    let mut store = HttpBrokerStore::with_executor(MockExecutor::new());

    // Queue some requests
    store.write(&path!(""), Record::parsed(to_value(&HttpRequest::get("https://example.com")).unwrap())).unwrap();
    store.write(&path!(""), Record::parsed(to_value(&HttpRequest::get("https://example.com")).unwrap())).unwrap();

    let record = store.read(&path!("outstanding")).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();

    if let Value::Map(map) = value {
        if let Some(Value::Array(items)) = map.get("items") {
            assert_eq!(items.len(), 2);
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

#[test]
fn request_status_uses_references() {
    let status = RequestStatus::complete("42".to_string());
    let value = to_value(&status).unwrap();

    if let Value::Map(map) = value {
        let response = map.get("response").unwrap();
        let reference = Reference::from_value(response).unwrap();
        assert_eq!(reference.path, "outstanding/42/response");
        assert_eq!(reference.type_info.as_ref().unwrap().name, "http-response");
    } else {
        panic!("Expected map");
    }
}

#[test]
fn docs_path_still_works() {
    let store = HttpBrokerStore::with_default_timeout().unwrap();
    let record = store.read(&path!("docs")).unwrap().unwrap();
    let value = record.into_value(&NoCodec).unwrap();

    // Docs should still return prose help
    if let Value::Map(map) = value {
        assert!(map.contains_key("title"));
        assert!(map.contains_key("description"));
    } else {
        panic!("Expected docs map");
    }
}
```

## Summary

| Store | Before | After |
|-------|--------|-------|
| Root | Nothing / Error | References to operations + docs |
| Outstanding | `[0, 1, 2]` | `{items: [refs...]}` |
| Handle Status | String paths | Reference objects |
| Actions | N/A | Typed descriptors at meta/ |
| Navigation | Construct from docs | Follow references |
| Docs | Prose help | Prose help (unchanged) |

Clients discover the API by following references from the root. Actions are
self-describing at meta/. Human-readable docs remain available for the help
system.

## Appendix: Type Vocabulary

Consistent type names across stores:

| Type Name | Meaning |
|-----------|---------|
| `collection` | Array of references to items |
| `action` | Write-only operation with accepts/returns |
| `request-handle` | Async request state + navigation |
| `http-request` | HttpRequest struct |
| `http-response` | HttpResponse struct |
| `accessor` | Read-only navigation point |
| `meta` | Meta namespace root |
| `docs` | Human-readable documentation |
