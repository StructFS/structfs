# Design: Layered StructFS Core

## The Layer Stack

```
┌─────────────────────────────────────────────────────────────┐
│  Application Layer                                          │
│  read_as::<User>(), write_as::<Config>()                   │
│  Rust types, serde integration                              │
├─────────────────────────────────────────────────────────────┤
│  Core Layer (StructFS)                                      │
│  Record (Raw | Parsed), Value, Path, Format                 │
│  Validated paths, structured values, format hints           │
├─────────────────────────────────────────────────────────────┤
│  Low-Level Layer (LLStructFS)                               │
│  Bytes, Vec<Bytes>, no validation                           │
│  Pure byte sequences, zero semantics                        │
├─────────────────────────────────────────────────────────────┤
│  Transport / FFI / Storage                                  │
│  WASM linear memory, TCP, files, shared memory              │
└─────────────────────────────────────────────────────────────┘
```

Each layer adds meaning. Each layer can be used independently. Bridges connect them.

## LLStructFS: The Narrow Waist

### Design Principles

1. **Bytes only**: No strings, no Unicode, no validation
2. **No allocation preferences**: Accept borrowed, return owned
3. **Object-safe**: Can be boxed, composed, sent across threads
4. **Zero semantics**: Path components are opaque byte sequences
5. **Minimal error model**: Transport-level errors only

### Core Types

```rust
// ll.rs - The entire LL layer

use bytes::Bytes;

/// A path component at the LL level - just bytes.
/// No validation, no semantics, no encoding assumptions.
pub type LLComponent = Bytes;

/// An owned path at the LL level - sequence of byte components.
pub type LLPath = Vec<Bytes>;

/// Errors at the LL level - transport/FFI focused.
#[derive(Debug)]
pub enum LLError {
    /// Generic I/O or transport failure
    Transport(Box<dyn std::error::Error + Send + Sync>),
    /// The operation cannot be performed (e.g., write to read-only)
    NotSupported,
    /// Resource limit exceeded (memory, handles, etc.)
    ResourceExhausted,
    /// Custom error code for protocol-specific errors
    Protocol { code: u32, detail: Bytes },
}

/// Read bytes from a path.
pub trait LLReader: Send + Sync {
    /// Read raw bytes from path components.
    ///
    /// Returns `Ok(None)` if the path doesn't exist.
    /// Returns `Ok(Some(bytes))` with the data.
    /// Returns `Err` only for transport/system failures.
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError>;
}

/// Write bytes to a path.
pub trait LLWriter: Send + Sync {
    /// Write raw bytes to path components.
    ///
    /// Returns the "result path" as a sequence of byte components.
    /// This may be the same as the input path, or different (e.g., generated ID).
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError>;
}

/// Combined read/write at LL level.
pub trait LLStore: LLReader + LLWriter {}
impl<T: LLReader + LLWriter> LLStore for T {}
```

That's it. ~50 lines. No dependencies except `bytes`.

### Why This Interface

**`&[&[u8]]` for input paths**:
- Zero-copy: caller provides references into their buffers
- Flexible: works with `&[&str]`, `&[&[u8]]`, `&[Bytes]` via AsRef
- Object-safe: concrete type, no generics

**`Bytes` for data**:
- Reference-counted: cheap cloning for forwarding
- Sliceable: zero-copy sub-ranges
- Thread-safe: `Send + Sync`

**`Vec<Bytes>` for result paths**:
- Owned: result outlives the call
- Each component is independently reference-counted
- Easy to convert to higher-level Path

**Minimal errors**:
- No semantic errors (invalid path format, type mismatch)
- Only transport-level failures
- `Protocol` variant for passing through protocol-specific codes

### Example: WASM FFI Adapter

```rust
/// A store implemented by a WASM module.
pub struct WasmLLStore {
    instance: wasmtime::Instance,
    memory: wasmtime::Memory,
    alloc_fn: wasmtime::TypedFunc<u32, u32>,
    free_fn: wasmtime::TypedFunc<u32, ()>,
    read_fn: wasmtime::TypedFunc<(u32, u32), u64>,  // (path_ptr, path_len) -> result
    write_fn: wasmtime::TypedFunc<(u32, u32, u32, u32), u64>,  // path + data -> result
}

impl LLReader for WasmLLStore {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Marshal path into WASM linear memory
        let path_ptr = self.marshal_path(path)?;
        let path_len = path.len() as u32;

        // Call into WASM
        let result = self.read_fn.call(&mut self.store, (path_ptr, path_len))
            .map_err(|e| LLError::Transport(Box::new(e)))?;

        // Unmarshal result (high 32 bits = ptr, low 32 = len, or sentinel for None)
        self.unmarshal_result(result)
    }
}

impl LLWriter for WasmLLStore {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        // Marshal path and data into WASM memory
        let path_ptr = self.marshal_path(path)?;
        let path_len = path.len() as u32;
        let data_ptr = self.marshal_bytes(&data)?;
        let data_len = data.len() as u32;

        // Call into WASM
        let result = self.write_fn.call(
            &mut self.store,
            (path_ptr, path_len, data_ptr, data_len)
        ).map_err(|e| LLError::Transport(Box::new(e)))?;

        // Unmarshal result path
        self.unmarshal_path_result(result)
    }
}
```

The WASM module sees raw bytes. It doesn't need to know about Path validation or Value types.

### Example: Network Transport

```rust
/// LL store over a TCP connection with simple framing.
pub struct TcpLLStore {
    stream: TcpStream,
}

impl LLReader for TcpLLStore {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Send: [1 byte: READ opcode] [path encoding]
        self.stream.write_all(&[0x01])?;  // READ
        self.write_path(path)?;
        self.stream.flush()?;

        // Receive: [1 byte: status] [optional: length-prefixed data]
        let mut status = [0u8; 1];
        self.stream.read_exact(&mut status)?;

        match status[0] {
            0x00 => Ok(None),  // NOT_FOUND
            0x01 => {          // OK
                let data = self.read_length_prefixed()?;
                Ok(Some(data))
            }
            code => Err(LLError::Protocol {
                code: code as u32,
                detail: Bytes::new()
            }),
        }
    }
}
```

The wire protocol is defined at the LL level. No JSON parsing, no Path validation. Just bytes moving.

### Example: Shared Memory IPC

```rust
/// LL store using shared memory for zero-copy IPC.
pub struct ShmLLStore {
    shm: SharedMemory,
    request_ring: RingBuffer,
    response_ring: RingBuffer,
}

impl LLReader for ShmLLStore {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Write request to ring buffer (path is copied in)
        let request_id = self.request_ring.write_read_request(path)?;

        // Wait for response
        let response = self.response_ring.wait_for(request_id)?;

        // Response data might be a direct slice of shared memory
        // Bytes can wrap this with custom drop logic
        Ok(response.data.map(|slice| {
            Bytes::from_owner(ShmSlice::new(self.shm.clone(), slice))
        }))
    }
}
```

True zero-copy: the returned `Bytes` points directly into shared memory.

## Bridging Layers

### LL → Core Bridge

Wraps an LL store to provide the Core interface:

```rust
use crate::ll::{LLReader, LLWriter, LLPath, LLError};
use crate::core::{Reader, Writer, Record, Path, Format, Error};

/// Adapts an LLStore to the Core Store interface.
pub struct LLToCore<T> {
    inner: T,
    /// Format hint for data read from LL layer (LL doesn't know formats)
    read_format: Format,
    /// Format to use when serializing for LL writes
    write_format: Format,
    codec: Box<dyn Codec>,
}

impl<T: LLReader> Reader for LLToCore<T> {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
        // Convert Path to &[&[u8]]
        let components: Vec<&[u8]> = from.components
            .iter()
            .map(|s| s.as_bytes())
            .collect();

        // Read via LL
        let bytes = match self.inner.ll_read(&components) {
            Ok(Some(b)) => b,
            Ok(None) => return Ok(None),
            Err(e) => return Err(Error::from_ll(e)),
        };

        // Wrap as Raw record with our format hint
        Ok(Some(Record::raw(bytes, self.read_format.clone())))
    }
}

impl<T: LLWriter> Writer for LLToCore<T> {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
        // Get bytes from Record (serialize if Parsed)
        let bytes = data.into_bytes(&*self.codec, &self.write_format)?;

        // Convert Path to &[&[u8]]
        let components: Vec<&[u8]> = to.components
            .iter()
            .map(|s| s.as_bytes())
            .collect();

        // Write via LL
        let result_path = self.inner.ll_write(&components, bytes)
            .map_err(Error::from_ll)?;

        // Convert result back to Path
        path_from_ll_components(result_path)
    }
}
```

### Core → LL Bridge

Wraps a Core store to provide the LL interface:

```rust
/// Adapts a Core Store to the LLStore interface.
pub struct CoreToLL<T> {
    inner: T,
    codec: Box<dyn Codec>,
    format: Format,
}

impl<T: Reader> LLReader for CoreToLL<T> {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Convert &[&[u8]] to Path (may fail if not valid UTF-8/identifiers)
        let path = match path_from_bytes(path) {
            Ok(p) => p,
            Err(_) => return Err(LLError::Protocol {
                code: 1,
                detail: Bytes::from_static(b"invalid path encoding"),
            }),
        };

        // Read via Core
        let record = match self.inner.read(&path) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(e) => return Err(LLError::Transport(Box::new(e))),
        };

        // Convert to bytes
        let bytes = record.into_bytes(&*self.codec, &self.format)
            .map_err(|e| LLError::Transport(Box::new(e)))?;

        Ok(Some(bytes))
    }
}

impl<T: Writer> LLWriter for CoreToLL<T> {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError> {
        // Convert path
        let path = path_from_bytes(path)
            .map_err(|_| LLError::Protocol {
                code: 1,
                detail: Bytes::from_static(b"invalid path encoding"),
            })?;

        // Wrap data as Raw record
        let record = Record::raw(data, self.format.clone());

        // Write via Core
        let result_path = self.inner.write(&path, record)
            .map_err(|e| LLError::Transport(Box::new(e)))?;

        // Convert result to LL path
        Ok(result_path.components
            .into_iter()
            .map(|s| Bytes::from(s.into_bytes()))
            .collect())
    }
}
```

### Helpers

```rust
/// Convert LL path components to Core Path.
fn path_from_bytes(components: &[&[u8]]) -> Result<Path, PathError> {
    let strings: Result<Vec<String>, _> = components
        .iter()
        .map(|b| std::str::from_utf8(b).map(|s| s.to_string()))
        .collect();

    let strings = strings.map_err(|_| PathError::InvalidEncoding)?;

    Path::from_components(strings)
}

/// Convert LL path (owned) to Core Path.
fn path_from_ll_components(components: Vec<Bytes>) -> Result<Path, Error> {
    let strings: Result<Vec<String>, _> = components
        .into_iter()
        .map(|b| String::from_utf8(b.to_vec()))
        .collect();

    let strings = strings.map_err(|_| Error::InvalidPath)?;

    Ok(Path { components: strings })
}
```

## The Complete Layer Definitions

### Layer 1: LLStructFS (~50 lines)

```rust
// packages/ll-store/src/lib.rs

use bytes::Bytes;

pub type LLPath = Vec<Bytes>;

#[derive(Debug)]
pub enum LLError {
    Transport(Box<dyn std::error::Error + Send + Sync>),
    NotSupported,
    ResourceExhausted,
    Protocol { code: u32, detail: Bytes },
}

pub trait LLReader: Send + Sync {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError>;
}

pub trait LLWriter: Send + Sync {
    fn ll_write(&mut self, path: &[&[u8]], data: Bytes) -> Result<LLPath, LLError>;
}

pub trait LLStore: LLReader + LLWriter {}
impl<T: LLReader + LLWriter> LLStore for T {}
```

### Layer 2: Core StructFS (~300 lines)

```rust
// packages/store/src/lib.rs

use bytes::Bytes;
use std::collections::BTreeMap;

// Value type for parsed data
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

// Path with validated components
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Path {
    pub components: Vec<String>,
}

// Format hint
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format(pub std::borrow::Cow<'static, str>);

// Record: maybe-parsed data
#[derive(Clone)]
pub enum Record {
    Raw { bytes: Bytes, format: Format },
    Parsed(Value),
}

// Errors at Core level
#[derive(Debug)]
pub enum Error {
    InvalidPath(PathError),
    NoRoute { path: Path },
    Codec { message: String },
    Ll(ll_store::LLError),
    // ... other semantic errors
}

pub trait Reader: Send + Sync {
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error>;
}

pub trait Writer: Send + Sync {
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error>;
}

pub trait Store: Reader + Writer {}
impl<T: Reader + Writer> Store for T {}

pub trait Codec: Send + Sync {
    fn decode(&self, bytes: &Bytes, format: &Format) -> Result<Value, Error>;
    fn encode(&self, value: &Value, format: &Format) -> Result<Bytes, Error>;
}
```

### Layer 3: Serde Integration (~100 lines)

```rust
// packages/store-serde/src/lib.rs

use serde::{Serialize, de::DeserializeOwned};
use structfs_store::{Reader, Writer, Record, Path, Error, Codec};

pub trait TypedReader: Reader {
    fn read_as<T: DeserializeOwned>(
        &mut self,
        from: &Path,
        codec: &dyn Codec,
    ) -> Result<Option<T>, Error> {
        let Some(record) = self.read(from)? else {
            return Ok(None);
        };
        let value = record.into_value(codec)?;
        // Convert Value to T via serde
        value_to_typed(value)
    }
}

impl<R: Reader> TypedReader for R {}

pub trait TypedWriter: Writer {
    fn write_as<T: Serialize>(
        &mut self,
        to: &Path,
        data: &T,
    ) -> Result<Path, Error> {
        let value = typed_to_value(data)?;
        self.write(to, Record::Parsed(value))
    }
}

impl<W: Writer> TypedWriter for W {}
```

## Composition Patterns

### LL-Level Composition

Routing at the LL level - no parsing anywhere:

```rust
pub struct LLOverlay {
    routes: Vec<(Vec<Bytes>, Box<dyn LLStore>)>,
}

impl LLReader for LLOverlay {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        for (prefix, store) in self.routes.iter_mut().rev() {
            if path_has_prefix_bytes(path, prefix) {
                let suffix = strip_prefix_bytes(path, prefix.len());
                return store.ll_read(&suffix);
            }
        }
        Ok(None)  // No route = not found
    }
}
```

Prefix matching is just byte comparison - no Unicode normalization.

### Mixed-Layer Composition

An LL store that delegates to a Core store for some paths:

```rust
pub struct HybridStore {
    /// Fast path: LL stores for high-throughput routes
    ll_routes: Vec<(Vec<Bytes>, Box<dyn LLStore>)>,
    /// Slow path: Core store for routes needing parsing
    core_fallback: CoreToLL<Box<dyn Store>>,
}

impl LLReader for HybridStore {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Try LL routes first (zero parsing)
        for (prefix, store) in self.ll_routes.iter_mut() {
            if path_has_prefix_bytes(path, prefix) {
                let suffix = strip_prefix_bytes(path, prefix.len());
                return store.ll_read(&suffix);
            }
        }

        // Fall back to Core (may parse)
        self.core_fallback.ll_read(path)
    }
}
```

### Protocol Gateway

A gateway that bridges gRPC (protobuf) and REST (JSON):

```rust
pub struct ProtocolGateway {
    // gRPC backends - LL level, protobuf bytes
    grpc_clients: HashMap<String, GrpcLLClient>,
    // REST backends - Core level, JSON
    rest_clients: HashMap<String, RestCoreClient>,
    // Codec for transcoding
    codec: MultiCodec,
}

impl LLReader for ProtocolGateway {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        let service = std::str::from_utf8(path.get(0).ok_or(LLError::NotSupported)?)
            .map_err(|_| LLError::Protocol { code: 1, detail: Bytes::new() })?;

        // gRPC route - pure LL, no parsing
        if let Some(client) = self.grpc_clients.get_mut(service) {
            return client.ll_read(&path[1..]);
        }

        // REST route - needs transcoding
        if let Some(client) = self.rest_clients.get_mut(service) {
            let path = path_from_bytes(&path[1..])?;
            let record = client.read(&path)?;

            // Transcode JSON → protobuf for the caller
            let bytes = record
                .ok_or(LLError::NotSupported)?
                .into_bytes(&self.codec, &Format::PROTOBUF)?;

            return Ok(Some(bytes));
        }

        Err(LLError::NotSupported)
    }
}
```

gRPC paths stay at LL level (pure forwarding). REST paths go through Core (parsing/transcoding).

## Memory Layout for FFI

For WASM and other FFI boundaries, define a standard memory layout:

```rust
/// Path encoding in linear memory:
///
/// [u32: component_count]
/// [Component]*
///
/// Component:
/// [u32: byte_length]
/// [u8: bytes...] (no padding)

/// Read request in linear memory:
/// [u32: path_ptr]
///
/// Read response in linear memory:
/// [u8: status]  // 0 = not found, 1 = ok, 2+ = error
/// [u32: length] // only if status == 1
/// [u8: data...] // only if status == 1

/// Write request in linear memory:
/// [u32: path_ptr]
/// [u32: data_length]
/// [u8: data...]
///
/// Write response in linear memory:
/// [u8: status]
/// [u32: result_path_ptr] // only if status == 1
```

This is language-agnostic. A C, Rust, Go, or AssemblyScript WASM module can implement it.

## Complete Example: End-to-End

```rust
fn main() {
    // 1. WASM module provides an LL store
    let wasm_store = WasmLLStore::load("user_service.wasm");

    // 2. Wrap it to get Core interface (adds Path validation)
    let core_store = LLToCore::new(
        wasm_store,
        Format::PROTOBUF,  // WASM module speaks protobuf
        Format::PROTOBUF,
        Box::new(ProtobufCodec::new(schema)),
    );

    // 3. Mount it in an overlay
    let mut overlay = OverlayStore::new();
    overlay.add_layer(path!("users"), core_store);

    // 4. Add a JSON-based local store
    overlay.add_layer(path!("config"), JsonFileStore::open("/etc/app"));

    // 5. Application uses typed interface
    let user: User = overlay.read_as(&path!("users/123"), &codec)?
        .ok_or(Error::NotFound)?;

    // Under the hood:
    // - path!("users/123") → Path { components: ["users", "123"] }
    // - overlay routes to core_store
    // - core_store converts to LL: &[b"123"]
    // - WasmLLStore calls into WASM, gets protobuf bytes back
    // - LLToCore wraps as Record::Raw { format: PROTOBUF }
    // - read_as() calls into_value() → parses protobuf → Value
    // - value_to_typed() converts Value → User
}
```

If the same request came via a gRPC gateway that just needs to forward:

```rust
impl LLReader for GrpcGateway {
    fn ll_read(&mut self, path: &[&[u8]]) -> Result<Option<Bytes>, LLError> {
        // Path: ["users", "123"]

        // Route to WASM store at LL level - no Core, no parsing
        self.wasm_store.ll_read(&path[1..])

        // Protobuf bytes flow through untouched
    }
}
```

Same WASM module, zero parsing overhead for the forwarding case.

## Summary

| Layer | Types | Operations | Semantics |
|-------|-------|------------|-----------|
| LL | `Bytes`, `&[&[u8]]` | `ll_read`, `ll_write` | None - pure bytes |
| Core | `Record`, `Value`, `Path` | `read`, `write` | Format hints, validated paths |
| Serde | Rust types | `read_as::<T>`, `write_as::<T>` | Type-safe access |

**Why three layers?**

- **LL**: WASM/FFI boundary, wire protocols, zero-copy forwarding
- **Core**: Application routing, format-aware caching, semantic operations
- **Serde**: Ergonomic typed access for application code

**Key insight**: Bridges between layers are explicit and auditable. You can see exactly where parsing happens. A pure-forwarding path never leaves the LL layer. A full-processing path goes LL → Core → Serde and back.

**Cost model**:
- LL-only path: O(1) per hop (byte slicing)
- LL → Core: O(path length) for path conversion
- Core parsing: O(data size)
- Core → Serde: O(data size) for type conversion

The narrow waist at LL means any transport (WASM, TCP, shared memory, files) can be a StructFS store without understanding paths or values.
