# Design: A Minimal Zero-Copy Core for StructFS

## The Additional Problem

The previous design (`minimal_core.md`) proposed `Value` as the universal representation. That's clean for application logic but forces parsing at every boundary:

```
gRPC Client → [parse protobuf] → Value → [serialize protobuf] → gRPC Server
```

For a proxy that's just routing based on path prefix, this is pure waste. The bytes come in, the bytes go out. Don't touch them.

## The Insight

There are two fundamentally different operations:

1. **Forwarding**: Route data based on path, don't inspect contents
2. **Processing**: Parse, inspect, transform, serialize

The core should support both. Pay the parsing cost only when you need to look inside.

## Proposal: Record as the Core Abstraction

### The Record Type

```rust
use bytes::Bytes;  // Reference-counted, zero-copy slicing

/// A record that can be forwarded without parsing or parsed for inspection.
#[derive(Clone)]
pub enum Record {
    /// Unparsed bytes with format hint. Zero-copy forwarding possible.
    Raw {
        bytes: Bytes,
        format: Format,
    },
    /// Parsed tree structure. Efficient for inspection and modification.
    Parsed(Value),
}
```

A `Record` is either raw bytes you can forward blindly, or a parsed tree you can inspect. You choose when to cross that boundary.

### Format Hints

```rust
/// Hint about the wire format of raw bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format(pub Cow<'static, str>);

impl Format {
    // Common formats as constants for efficiency
    pub const JSON: Format = Format(Cow::Borrowed("application/json"));
    pub const PROTOBUF: Format = Format(Cow::Borrowed("application/protobuf"));
    pub const MSGPACK: Format = Format(Cow::Borrowed("application/msgpack"));
    pub const CBOR: Format = Format(Cow::Borrowed("application/cbor"));
    pub const OCTET_STREAM: Format = Format(Cow::Borrowed("application/octet-stream"));

    /// For Value that was never serialized
    pub const VALUE: Format = Format(Cow::Borrowed("application/x-structfs-value"));

    pub fn custom(s: impl Into<String>) -> Self {
        Format(Cow::Owned(s.into()))
    }
}
```

Format is a hint, not a contract. A proxy doesn't validate—it forwards. The receiver knows what to expect.

### Core Traits

```rust
pub trait Reader {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error>;
}

pub trait Writer {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error>;
}

pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}
```

Same simple traits as before, but now `Record` can be forwarded without parsing.

### Record Operations

```rust
impl Record {
    // === Construction ===

    /// Create from raw bytes.
    pub fn raw(bytes: impl Into<Bytes>, format: Format) -> Self {
        Record::Raw { bytes: bytes.into(), format }
    }

    /// Create from a parsed value.
    pub fn parsed(value: Value) -> Self {
        Record::Parsed(value)
    }

    // === Inspection (cheap) ===

    /// Is this record in raw (unparsed) form?
    pub fn is_raw(&self) -> bool {
        matches!(self, Record::Raw { .. })
    }

    /// Get the format hint. For Parsed values, returns VALUE.
    pub fn format(&self) -> Format {
        match self {
            Record::Raw { format, .. } => format.clone(),
            Record::Parsed(_) => Format::VALUE,
        }
    }

    /// Get raw bytes if available without serialization.
    pub fn as_bytes(&self) -> Option<&Bytes> {
        match self {
            Record::Raw { bytes, .. } => Some(bytes),
            Record::Parsed(_) => None,
        }
    }

    /// Get parsed value if available without parsing.
    pub fn as_value(&self) -> Option<&Value> {
        match self {
            Record::Raw { .. } => None,
            Record::Parsed(v) => Some(v),
        }
    }

    // === Conversion (potentially costly) ===

    /// Parse into a Value. No-op if already parsed.
    ///
    /// This is where you pay the parsing cost.
    pub fn into_value(self, codec: &dyn Codec) -> Result<Value, Error> {
        match self {
            Record::Parsed(v) => Ok(v),
            Record::Raw { bytes, format } => codec.decode(&bytes, &format),
        }
    }

    /// Serialize into bytes. No-op if already raw.
    ///
    /// This is where you pay the serialization cost.
    pub fn into_bytes(self, codec: &dyn Codec, target_format: &Format) -> Result<Bytes, Error> {
        match self {
            Record::Raw { bytes, format } if &format == target_format => Ok(bytes),
            Record::Raw { bytes, format } => {
                // Transcode: parse then re-serialize
                let value = codec.decode(&bytes, &format)?;
                codec.encode(&value, target_format)
            }
            Record::Parsed(v) => codec.encode(&v, target_format),
        }
    }
}
```

### The Codec Trait

Parsing and serialization are pluggable:

```rust
/// Handles encoding/decoding between Value and wire formats.
pub trait Codec: Send + Sync {
    /// Decode raw bytes into a Value.
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error>;

    /// Encode a Value into raw bytes.
    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error>;

    /// Check if this codec supports a format.
    fn supports(&self, format: &Format) -> bool;
}
```

A default codec handles JSON, MessagePack, CBOR. Custom codecs can handle protobuf (with schema), Avro, etc.

```rust
/// Default codec supporting common self-describing formats.
pub struct DefaultCodec;

impl Codec for DefaultCodec {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error> {
        match format.0.as_ref() {
            "application/json" => json_decode(bytes),
            "application/msgpack" => msgpack_decode(bytes),
            "application/cbor" => cbor_decode(bytes),
            _ => Err(Error::UnsupportedFormat(format.clone())),
        }
    }

    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error> {
        match format.0.as_ref() {
            "application/json" => json_encode(value),
            "application/msgpack" => msgpack_encode(value),
            "application/cbor" => cbor_encode(value),
            _ => Err(Error::UnsupportedFormat(format.clone())),
        }
    }

    fn supports(&self, format: &Format) -> bool {
        matches!(format.0.as_ref(),
            "application/json" | "application/msgpack" | "application/cbor")
    }
}
```

## Use Cases

### 1. Zero-Copy Forwarding Proxy

A gRPC proxy that routes based on path prefix:

```rust
struct GrpcProxy {
    routes: HashMap<Path, GrpcClient>,
}

impl Writer for GrpcProxy {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // Find the route - just path matching, no parsing
        let (prefix, client) = self.find_route(to)?;
        let suffix = to.strip_prefix(&prefix).unwrap();

        // Forward the record as-is. If it came in as Raw protobuf,
        // it goes out as Raw protobuf. Zero parsing.
        client.write(&suffix, data)
    }
}
```

The protobuf bytes flow through untouched. The proxy never calls `into_value()`.

### 2. Format-Agnostic Caching Proxy

A cache that stores whatever format comes in:

```rust
struct CachingProxy {
    cache: HashMap<Path, Record>,
    upstream: Box<dyn Store>,
}

impl Reader for CachingProxy {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Check cache first
        if let Some(record) = self.cache.get(from) {
            return Ok(Some(record.clone()));  // Bytes clone is cheap (refcount)
        }

        // Fetch from upstream
        if let Some(record) = self.upstream.read(from)? {
            self.cache.insert(from.clone(), record.clone());
            return Ok(Some(record));
        }

        Ok(None)
    }
}
```

The cache doesn't care if it's storing JSON, protobuf, or parsed Values. It just holds Records.

### 3. JSON-to-Protobuf Transformer

When you actually need to transform:

```rust
struct JsonToProtobufTransformer {
    source: Box<dyn Store>,
    codec: Box<dyn Codec>,  // Includes protobuf support
}

impl Reader for JsonToProtobufTransformer {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        let Some(record) = self.source.read(from)? else {
            return Ok(None);
        };

        // Now we pay the cost: parse (if needed) and re-serialize
        let bytes = record.into_bytes(&*self.codec, &Format::PROTOBUF)?;

        Ok(Some(Record::raw(bytes, Format::PROTOBUF)))
    }
}
```

The transformer explicitly converts. The cost is visible and intentional.

### 4. Application Logic with Typed Access

For application code that wants structured data:

```rust
fn process_user(store: &mut dyn Store, codec: &dyn Codec) -> Result<(), Error> {
    let Some(record) = store.read(&path!("users/1"))? else {
        return Err(Error::NotFound);
    };

    // Parse the record into a Value
    let value = record.into_value(codec)?;

    // Work with structured data
    let name = value.get(&path!("name"))
        .ok_or(Error::MissingField("name"))?;

    // ... business logic ...

    Ok(())
}
```

With serde integration (optional layer):

```rust
use structfs_serde::TypedRecord;

fn process_user(store: &mut dyn Store, codec: &dyn Codec) -> Result<(), Error> {
    let Some(record) = store.read(&path!("users/1"))? else {
        return Err(Error::NotFound);
    };

    // Parse directly into a Rust type
    let user: User = record.into_typed(codec)?;

    // ... business logic ...

    Ok(())
}
```

### 5. Sub-Path Access

What about reading `users/1/name` when the store holds complete user records?

```rust
impl Reader for UserStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // This store holds user records at "users/{id}"
        if from.len() < 2 || from[0] != "users" {
            return Ok(None);
        }

        let user_id = &from[1];
        let raw_record = self.fetch_user(user_id)?;  // Returns Record::Raw

        if from.len() == 2 {
            // Reading the whole user - return raw, no parsing
            return Ok(Some(raw_record));
        }

        // Reading a sub-path - must parse to navigate
        let sub_path = from.slice_as_path(2, from.len());
        let value = raw_record.into_value(&self.codec)?;

        match value.get(&sub_path) {
            Some(v) => Ok(Some(Record::parsed(v.clone()))),
            None => Ok(None),
        }
    }
}
```

Sub-path access forces parsing. That's inherent—you can't navigate a tree without parsing it. But whole-record access stays zero-copy.

## The OverlayStore

Composition remains simple because routing doesn't require parsing:

```rust
pub struct OverlayStore {
    layers: Vec<(Path, Box<dyn Store + Send + Sync>)>,
}

impl Reader for OverlayStore {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        for (prefix, store) in self.layers.iter_mut().rev() {
            if from.has_prefix(prefix) {
                let suffix = from.strip_prefix(prefix).unwrap();
                // Forward without parsing - store returns whatever format it has
                return store.read(&suffix);
            }
        }
        Err(Error::NoRoute { path: from.clone() })
    }
}

impl Writer for OverlayStore {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        for (prefix, store) in self.layers.iter_mut().rev() {
            if to.has_prefix(prefix) {
                let suffix = to.strip_prefix(prefix).unwrap();
                // Forward without parsing
                return store.write(&suffix, data);
            }
        }
        Err(Error::NoRoute { path: to.clone() })
    }
}
```

A request can flow through multiple overlay layers without ever being parsed.

## Cost Model

| Operation | Cost |
|-----------|------|
| `Record::raw(bytes, format)` | O(1) - just wraps |
| `Record::parsed(value)` | O(1) - just wraps |
| `record.clone()` (Raw) | O(1) - Bytes is refcounted |
| `record.clone()` (Parsed) | O(n) - deep clone of Value tree |
| `record.as_bytes()` | O(1) - returns reference |
| `record.as_value()` | O(1) - returns reference |
| `record.into_value()` (already Parsed) | O(1) |
| `record.into_value()` (Raw) | O(n) - parsing cost |
| `record.into_bytes()` (already Raw, same format) | O(1) |
| `record.into_bytes()` (needs serialization) | O(n) |
| Forwarding through OverlayStore | O(layers) - no parsing |

## Lazy Parsing Variant

For cases where you might or might not need to parse:

```rust
use std::sync::OnceLock;

/// A record with lazy parsing. Thread-safe.
pub struct LazyRecord {
    raw: Option<(Bytes, Format)>,
    parsed: OnceLock<Value>,
}

impl LazyRecord {
    pub fn from_raw(bytes: Bytes, format: Format) -> Self {
        Self {
            raw: Some((bytes, format)),
            parsed: OnceLock::new(),
        }
    }

    pub fn from_parsed(value: Value) -> Self {
        let lock = OnceLock::new();
        lock.set(value).unwrap();
        Self {
            raw: None,
            parsed: lock,
        }
    }

    /// Get the value, parsing if necessary. Caches the result.
    pub fn value(&self, codec: &dyn Codec) -> Result<&Value, Error> {
        self.parsed.get_or_try_init(|| {
            let (bytes, format) = self.raw.as_ref()
                .ok_or(Error::Internal("no raw data to parse"))?;
            codec.decode(bytes, format)
        })
    }

    /// Get raw bytes if available without serialization.
    pub fn bytes(&self) -> Option<&Bytes> {
        self.raw.as_ref().map(|(b, _)| b)
    }
}
```

This is useful for middleware that *might* need to inspect data:

```rust
fn maybe_transform(record: &LazyRecord, codec: &dyn Codec) -> Result<bool, Error> {
    // Check a header first (cheap)
    if !should_transform(record.bytes()) {
        return Ok(false);  // No parsing happened
    }

    // Now parse (expensive, but only when needed)
    let value = record.value(codec)?;
    transform(value)?;
    Ok(true)
}
```

## Comparison to Previous Design

| Aspect | Value-Only Design | Record Design |
|--------|-------------------|---------------|
| Core type | `Value` | `Record` (Raw or Parsed) |
| Forwarding cost | O(n) parse + O(n) serialize | O(1) |
| Application code | Direct Value access | Call `.into_value()` |
| Format awareness | None (format-agnostic) | Explicit format hints |
| Complexity | Simpler | Slightly more complex |
| Zero-copy possible | No | Yes |

## What This Enables

1. **Protocol-agnostic proxies**: Route gRPC, REST, GraphQL through the same infrastructure
2. **Format-preserving caches**: Cache protobuf as protobuf, JSON as JSON
3. **Lazy transformation**: Only parse when you actually need to inspect
4. **Efficient pipelines**: Chain middleware without repeated parse/serialize cycles
5. **Mixed-format systems**: Some paths serve JSON, others protobuf, transparently

## Open Questions

### 1. Should Record be an enum or a struct with Option fields?

Enum (current proposal):
```rust
enum Record {
    Raw { bytes: Bytes, format: Format },
    Parsed(Value),
}
```

Struct:
```rust
struct Record {
    bytes: Option<Bytes>,
    format: Format,
    value: Option<Value>,
}
```

The struct allows having both simultaneously (useful for caching), but is less clear about invariants. Recommend: keep the enum, add `LazyRecord` for the caching case.

### 2. Should Codec be in core or a separate layer?

Arguments for core:
- `into_value()` needs a codec
- Common operation

Arguments for separate:
- Keeps core minimal
- Different applications need different codecs

Recommendation: Core defines the `Codec` trait. Default implementations live in a separate crate. The core provides a `NoCodec` that returns errors, forcing explicit codec selection.

### 3. How to handle streaming large records?

Out of scope for this design. Large data should use References (as mentioned in the original design) pointing to chunked storage. Each chunk is a normal-sized Record.

### 4. What about borrowing instead of cloning?

The `Bytes` type already handles this via reference counting. For `Value`, we could add:

```rust
impl Record {
    fn value_ref(&self, codec: &dyn Codec) -> Result<Cow<Value>, Error>;
}
```

This returns a borrowed reference for Parsed, owned for Raw (which must parse). Worth adding if profiling shows clone overhead.

## Summary

The zero-copy design adds one concept: `Record` as either raw bytes or parsed value. This single addition enables:

- O(1) forwarding through middleware chains
- Format-preserving caching and proxying
- Explicit, visible parsing costs
- Mixed-format systems

The core remains simple:
- `Path` - addressing
- `Value` - parsed tree structure
- `Record` - maybe-parsed data with format hint
- `Reader`/`Writer` - the two operations
- `Codec` - pluggable parsing/serialization

Total complexity increase over Value-only design: one enum, one trait. Worth it for the performance characteristics of real proxy/middleware systems.
