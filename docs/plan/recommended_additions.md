# Recommended Additions Plan

This plan outlines the remaining work to complete the StructFS implementation after the core refactoring. Items are ordered by dependency and practical value.

---

## 1. LocalDiskStore

**Priority:** High
**Complexity:** Medium
**Dependencies:** None

The factory currently returns an error for `MountConfig::Local`. This is the most requested missing feature.

### Design

```rust
// packages/json_store/src/local.rs
pub struct LocalDiskStore {
    root: PathBuf,
}
```

Files on disk map to StructFS paths:
- `/data/users/123` → `{root}/data/users/123.json`
- Directories become implicit (created on write, listed on read of parent)

### Implementation Steps

1. Create `packages/json_store/src/local.rs`
2. Implement `Reader`:
   - Read file as bytes
   - Return `Record::Raw` with `Format::JSON`
   - Directory read returns array of child names
3. Implement `Writer`:
   - Parse Record to Value
   - Serialize to JSON
   - Write atomically (write to temp, rename)
   - Create parent directories as needed
4. Add to factory in `store_context.rs`
5. Add tests for:
   - Basic read/write round-trip
   - Directory listing
   - Atomic write (crash safety)
   - Path traversal prevention (`../` attacks)

### Security Considerations

- Validate that resolved paths stay within `root`
- Use `canonicalize()` to detect symlink escapes
- Reject paths with null bytes or other invalid characters

---

## 3. Additional Codecs

**Priority:** Medium
**Complexity:** Low
**Dependencies:** None

JSON is the only codec. Add binary formats for efficiency.

### Design

```rust
// packages/serde-store/src/codec.rs
pub struct CborCodec;
pub struct MessagePackCodec;
pub struct DefaultCodec; // Tries JSON, CBOR, MessagePack based on format hint
```

### Implementation Steps

1. Add `ciborium` dependency for CBOR
2. Add `rmp-serde` dependency for MessagePack
3. Implement `Codec` for each:
   - `decode`: deserialize bytes to Value
   - `encode`: serialize Value to bytes
   - `supports`: check format constant
4. Create `DefaultCodec` that chains codecs:
   ```rust
   impl Codec for DefaultCodec {
       fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
           if format == &Format::JSON { return JsonCodec.decode(bytes, format); }
           if format == &Format::CBOR { return CborCodec.decode(bytes, format); }
           // ...
       }
   }
   ```
5. Add integration tests with round-trip verification
6. Update `NoCodec` documentation to recommend `DefaultCodec` for most uses

### Format Detection

For `Format::OCTET_STREAM` (unknown format), try in order:
1. JSON (check for `{` or `[` prefix)
2. CBOR (check for CBOR magic byte)
3. MessagePack (fallback)

---

## 4. True Async I/O

**Priority:** Medium
**Complexity:** Medium
**Dependencies:** Codecs (for async file parsing)

The async broker uses `std::thread::spawn`. Replace with proper async.

### Design

Feature-gated async implementations:

```toml
[features]
async = ["tokio", "async-trait"]
async-fs = ["async", "tokio/fs"]
```

### Implementation Steps

1. **Async HTTP Broker** (`packages/http/src/async_core.rs`):
   - Replace `std::thread::spawn` with `tokio::spawn`
   - Use `reqwest::Client` (async version)
   - Store handles in `Arc<Mutex<_>>` for cross-task access

2. **Async FS Store** (new file):
   - Use `tokio::fs` for file operations
   - Implement `AsyncReader` and `AsyncWriter`
   - Buffer reads/writes appropriately

3. **Update SyncToAsync adapter**:
   - Use `tokio::task::spawn_blocking` for CPU-bound work
   - Keep `Arc<Mutex<_>>` pattern for simple cases

4. **Integration tests**:
   - Concurrent reads to same store
   - Write-then-read consistency
   - Timeout handling

### Considerations

- Don't make async mandatory; sync should remain the default
- Use `#[cfg(feature = "async")]` guards
- Document when to use sync vs async

---

## 5. Remote StructFS Client

**Priority:** Medium
**Complexity:** Medium
**Dependencies:** Async I/O (optional but recommended)

Enable mounting remote StructFS instances.

### Wire Protocol

Simple JSON-over-HTTP:

```
POST /write?path=/users/123
Content-Type: application/json
{"name": "Alice"}

Response:
{"result_path": "/users/123"}
```

```
GET /read?path=/users/123

Response:
{"name": "Alice"}
```

```
DELETE /delete?path=/users/123

Response:
{"ok": true}
```

### Implementation Steps

1. **Define protocol types** (`packages/http/src/protocol.rs`):
   ```rust
   struct ReadRequest { path: String }
   struct ReadResponse { value: Option<Value>, error: Option<String> }
   struct WriteRequest { path: String, value: Value }
   struct WriteResponse { result_path: String, error: Option<String> }
   ```

2. **Client store** (`packages/http/src/remote.rs`):
   - Implement `Reader`: GET to `/read?path=...`
   - Implement `Writer`: POST to `/write?path=...`
   - Implement `delete`: DELETE to `/delete?path=...`
   - Handle errors from server

3. **Server handler** (`packages/http/src/server.rs`):
   - Wrap any `Store` and expose via HTTP
   - Parse requests, call store, serialize responses
   - Content negotiation for formats

4. **Add to factory**:
   ```rust
   MountConfig::Structfs { url } => {
       Ok(Box::new(RemoteStructFsStore::new(url)?))
   }
   ```

5. **Integration tests**:
   - Start local server, mount as remote
   - Read/write round-trip
   - Error propagation

### Future Extensions

- WebSocket for streaming reads
- Authentication headers
- Compression (gzip, brotli)

---

## 6. Benchmarks

**Priority:** Low
**Complexity:** Low
**Dependencies:** All of the above (for comprehensive benchmarks)

Prove the design claims with numbers.

### Benchmark Suite

Create `packages/bench/` with criterion benchmarks:

1. **Forwarding overhead**:
   - Create N overlay layers
   - Forward Raw record through all layers
   - Compare: 1 layer vs 5 vs 10 vs 20
   - Expected: O(N) but with tiny constant factor

2. **Parse vs forward**:
   - Same setup
   - Compare: forward Raw vs parse-then-forward
   - Expected: Forward is 10-100x faster for large records

3. **LazyRecord caching**:
   - Access `.value()` multiple times
   - Compare: first access vs subsequent
   - Expected: Subsequent is ~0 cost

4. **Codec performance**:
   - Encode/decode same data with JSON vs CBOR vs MessagePack
   - Compare: throughput and latency
   - Expected: Binary formats 2-5x faster

5. **Concurrent access**:
   - Multiple threads reading/writing
   - Compare: sync with locks vs async
   - Expected: Async scales better with contention

### Implementation Steps

1. Add `packages/bench/Cargo.toml` with criterion
2. Create benchmark files for each category
3. Add `cargo bench` to CI (track regressions)
4. Document results in `docs/performance.md`

---

## Implementation Order

Recommended sequence based on dependencies and value:

```
Phase 1 (Foundation):
├── Delete semantics (unblocks clean store APIs)
├── LocalDiskStore (most requested feature)
└── Additional codecs (enables binary formats)

Phase 2 (Network):
├── Remote StructFS client (enables distribution)
└── True async I/O (enables scale)

Phase 3 (Validation):
└── Benchmarks (proves the design)
```

Each phase can be merged independently. No phase depends on a later phase.

---

## Estimated Effort

| Item | Files | Lines | Days |
|------|-------|-------|------|
| Delete semantics | 5-6 | ~100 | 1 |
| LocalDiskStore | 2-3 | ~300 | 2 |
| Additional codecs | 2 | ~200 | 1 |
| True async I/O | 3-4 | ~400 | 2-3 |
| Remote client | 4-5 | ~500 | 3 |
| Benchmarks | 5-6 | ~400 | 2 |

Total: ~2000 lines, ~12 days of focused work.

---

## Non-Goals (For Now)

These are explicitly deferred:

- **Transactions**: Atomic multi-path operations. Complex, not needed yet.
- **Subscriptions**: Watch for changes. Requires event system.
- **Compression**: Per-record compression. Can layer on later.
- **Encryption**: At-rest encryption. Security audit needed first.
- **Replication**: Multi-node consistency. Much larger scope.

These can be revisited after the core is stable and benchmarked.
