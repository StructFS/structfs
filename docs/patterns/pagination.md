# The Pagination Pattern

A **paginated collection** returns items in discrete pages with hypermedia links
for navigation. Clients never construct pagination URLs—they follow References
provided by the server.

## Core Principle

**The server controls navigation.** Every paginated response includes References
to adjacent pages. The cursor is embedded in the path, not in query parameters.
Clients iterate by following `links.next` until it's absent.

## Why Cursor-Based

StructFS pagination uses **cursors**, not offsets. Offsets break under
concurrent modification:

```
Page 1: items 0-9
# Someone deletes item 5
Page 2: items 10-19  # You just skipped what was item 11
```

Cursors are stable:

```
Page 1: items up to cursor "abc123"
# Someone deletes an item
Page 2: items after cursor "abc123"  # Still correct
```

The cursor is opaque to the client. It could be an ID, a timestamp, a composite
key—the client doesn't care. It just follows the reference.

## Response Structure

A paginated collection response:

```json
{
  "items": [
    {"path": "handles/0", "type": {"name": "handle"}},
    {"path": "handles/1", "type": {"name": "handle"}},
    {"path": "handles/2", "type": {"name": "handle"}}
  ],
  "page": {
    "size": 3
  },
  "links": {
    "next": {"path": "handles/after/2/limit/3"},
    "self": {"path": "handles/limit/3"}
  }
}
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `items` | array | yes | References or inline values |
| `page.size` | integer | yes | Number of items in this response |
| `page.total` | integer | no | Total items in collection (omit if expensive/unknown) |
| `page.remaining` | integer | no | Items after this page (omit if unknown) |
| `links.next` | Reference | no | Next page (absent on last page) |
| `links.prev` | Reference | no | Previous page (absent on first page) |
| `links.first` | Reference | no | First page |
| `links.self` | Reference | yes | This page (for caching/bookmarking) |

## Path Structure

Pagination parameters are path components, not query strings:

```
/collection                          # First page, default size
/collection/limit/50                 # First page, size 50
/collection/after/{cursor}           # After cursor, default size
/collection/after/{cursor}/limit/50  # After cursor, size 50
/collection/before/{cursor}          # Before cursor (reverse navigation)
/collection/before/{cursor}/limit/50
```

### Why Paths, Not Query Strings?

1. StructFS has paths, not URLs. No query string concept.
2. Paths are validated—components are Unicode identifiers.
3. Caching and routing work on path prefixes.
4. The cursor *is* an address. It belongs in the address.

## Cursor Formats

The cursor is opaque to clients but stores choose a format:

### Keyed Cursor

Recommended for sorted data:

```
/users/after/user_12345/limit/20
```

The cursor is the last item's key. Simple, debuggable, stable.

### Opaque Cursor

For complex ordering or privacy:

```
/feed/after/eyJ0IjoxNzA0MDY3MjAwfQ/limit/20
```

Base64-encoded state. Client can't interpret it. Server can embed timestamp,
composite keys, shard information, etc.

### Composite Cursor

For multi-field ordering:

```
/events/after/2024-01-01/event_500/limit/20
```

Multiple path components for compound sort keys.

## Empty and Final Pages

### Empty Collection

```json
{
  "items": [],
  "page": {"size": 0},
  "links": {
    "self": {"path": "handles/limit/20"}
  }
}
```

No `next` or `prev` links.

### Last Page

```json
{
  "items": [
    {"path": "handles/98"},
    {"path": "handles/99"}
  ],
  "page": {"size": 2},
  "links": {
    "prev": {"path": "handles/before/98/limit/20"},
    "first": {"path": "handles/limit/20"},
    "self": {"path": "handles/after/97/limit/20"}
  }
}
```

No `next` link signals end of collection.

## Inline vs. Reference Items

Stores choose whether to inline values or return references:

### References (Shallow)

```json
{
  "items": [
    {"path": "users/alice", "type": {"name": "user"}},
    {"path": "users/bob", "type": {"name": "user"}}
  ]
}
```

Client follows references for full data. Good for large items or when client
may not need all fields.

### Inline (Deep)

```json
{
  "items": [
    {"id": "alice", "name": "Alice", "email": "alice@example.com"},
    {"id": "bob", "name": "Bob", "email": "bob@example.com"}
  ]
}
```

Full data in response. Good for small items, list views.

### Mixed

```json
{
  "items": [
    {
      "id": "alice",
      "name": "Alice",
      "profile": {"path": "users/alice/profile", "type": {"name": "profile"}}
    }
  ]
}
```

Summary inline, details as references. Best of both.

## Meta Lens for Pagination

`meta/collection` describes pagination capabilities:

```json
{
  "paginated": true,
  "cursor": {
    "type": "keyed",
    "field": "id"
  },
  "limits": {
    "default": 20,
    "max": 100
  },
  "capabilities": {
    "forward": true,
    "backward": true,
    "total_count": false
  },
  "ordering": {
    "default": "created_desc",
    "available": ["created_asc", "created_desc", "name_asc"]
  }
}
```

This tells clients:

- How pagination works (keyed on `id`)
- What page sizes are allowed
- Whether reverse navigation is supported
- Whether total count is available (expensive for some stores)
- What orderings are available

## Requesting Different Orderings

Ordering is a path component:

```
/users/by/created_desc/limit/20
/users/by/name_asc/after/bob/limit/20
```

The `by/{ordering}` lens changes the sort order. The cursor is interpreted in
that ordering's context.

## Writing to Paginated Collections

Reading navigates. Writing appends or modifies.

### Append to Collection

```
write /users {"name": "Charlie", "email": "charlie@example.com"}
→ {"path": "users/charlie"}
```

Returns a reference to the created item.

### Bulk Append

```
write /users/batch [
  {"name": "Dave"},
  {"name": "Eve"}
]
→ {
  "items": [
    {"path": "users/dave"},
    {"path": "users/eve"}
  ]
}
```

Returns references to all created items.

## Streaming Collections

Some collections have no end—logs, event streams, real-time feeds.

```json
{
  "items": [
    {"path": "events/1001"},
    {"path": "events/1002"}
  ],
  "page": {"size": 2},
  "links": {
    "next": {"path": "events/after/1002/limit/2"},
    "live": {"path": "events/after/1002/live"}
  }
}
```

The `live` link indicates the collection is unbounded. Reading from it may
block until new items arrive (following StructFS's "read can block" pattern
for live data).

## Stable Pages via Snapshots

For consistency during iteration, stores can offer snapshot pagination:

```
write /users/snapshot {}
→ {"path": "users/snapshots/abc123"}

read /users/snapshots/abc123/limit/20
→ {items: [...], links: {next: {path: "users/snapshots/abc123/after/..."}}}
```

The snapshot captures collection state at write time. Subsequent reads
paginate over that frozen state.

## Error Cases

### Invalid Cursor

```json
{
  "error": {
    "type": "invalid_cursor",
    "message": "Cursor 'xyz' not found or expired",
    "links": {
      "first": {"path": "users/limit/20"}
    }
  }
}
```

Always provide a recovery path.

### Limit Too Large

```json
{
  "error": {
    "type": "limit_exceeded",
    "message": "Maximum page size is 100",
    "max": 100,
    "links": {
      "valid": {"path": "users/limit/100"}
    }
  }
}
```

## Rust Implementation

```rust
use serde::{Deserialize, Serialize};

/// A paginated response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    /// Items in this page.
    pub items: Vec<T>,

    /// Page metadata.
    pub page: PageInfo,

    /// Navigation links.
    pub links: PageLinks,
}

/// Metadata about the current page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageInfo {
    /// Number of items in this response.
    pub size: usize,

    /// Total items in collection (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,

    /// Items remaining after this page (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<usize>,
}

/// Navigation links for pagination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageLinks {
    /// This page (for caching/bookmarking).
    #[serde(rename = "self")]
    pub self_link: Reference,

    /// Next page (absent on last page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Reference>,

    /// Previous page (absent on first page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<Reference>,

    /// First page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<Reference>,
}

impl<T> Page<T> {
    /// Check if this is the last page.
    pub fn is_last(&self) -> bool {
        self.links.next.is_none()
    }

    /// Check if this is the first page.
    pub fn is_first(&self) -> bool {
        self.links.prev.is_none()
    }
}
```

## Client Iteration

The client loop is simple:

```rust
let mut page: Page<Reference> = store.read(&Path::parse("users/limit/20")?)?;
loop {
    for item in &page.items {
        process(item);
    }
    match &page.links.next {
        Some(next) => page = store.read(&next.path)?,
        None => break,
    }
}
```

No URL construction. No parameter encoding. Just follow the references.

## Summary

| Principle | Implementation |
|-----------|----------------|
| Server controls navigation | `links.next`, `links.prev` are References |
| Cursors, not offsets | Path encodes cursor: `/after/{cursor}` |
| No URL construction | Client only follows References |
| Path-based parameters | `/limit/50`, `/by/name_asc` |
| Self-describing | `meta/collection` describes capabilities |
| Graceful degradation | Errors include recovery links |
