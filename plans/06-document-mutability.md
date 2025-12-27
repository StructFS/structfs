# Plan 6: Document &mut self Decision

## Status: STILL RELEVANT

This is a documentation task. The `&mut self` decision is correct, but it should be documented for future maintainers.

## Current State (2025-12-27)

The codebase uses `&mut self` for `Reader::read()` and `Writer::write()`. This is intentional but not documented. The documentation explaining the rationale should be added.

## The Decision

**Keep `&mut self` in Reader/Writer traits.** This is intentional, not accidental.

## Analysis Summary

Stores that **need** `&mut self` for reads:
- `HttpBrokerStore` - Executes and caches response on first read
- `FsStore` - Updates file position on read
- `OverlayStore` - Routes to mutable child stores

Stores that **don't need** `&mut self` for reads (but accept it):
- `InMemoryStore` - Pure read from BTreeMap
- `EnvStore` - Reads from system environment
- `TimeStore` - Reads system time
- `RandomStore` - Generates random values
- `ProcStore` - Reads process info
- `HttpClientStore` - Stateless HTTP requests

## Rationale

1. **Some stores genuinely need mutation on read:**
   - HTTP broker executes and caches responses
   - Filesystem tracks position
   - Overlay routes to mutable children

2. **Simplicity over granularity:**
   - Single trait signature works uniformly
   - No need for `ImmutableReader` vs `MutableReader` split
   - Blanket impls work seamlessly (`&mut T`, `Box<T>`)

3. **Interior mutability avoided:**
   - No `Mutex<T>` or `RefCell<T>` overhead
   - No runtime borrow checking
   - Clear ownership at compile time

4. **Concurrent access is a separate concern:**
   - Use `Arc<Mutex<dyn Reader>>` when needed
   - Synchronization is explicit at call site
   - Not hidden in trait design

## Alternative Considered (Not Taken)

Split into two traits:

```rust
pub trait ImmutableReader: Send + Sync {
    fn read(&self, from: &Path) -> Result<Option<Record>, Error>;
}

pub trait MutableReader: Send + Sync {
    fn read_mut(&mut self, from: &Path) -> Result<Option<Record>, Error>;
}
```

**Why not:**
- Doubles API surface
- Complicates generic code
- Forces stores to decide upfront
- Blanket impls become complex
- Marginal benefit for REPL use case

## Documentation to Add

### Step 1: Add trait documentation

**File:** `packages/core-store/src/lib.rs`

```rust
/// Read data from a store at the given path.
///
/// # Mutability
///
/// Both `Reader::read` and `Writer::write` take `&mut self`. This is intentional:
///
/// 1. **Stateful stores exist**: Some stores maintain state that changes on read.
///    For example:
///    - HTTP broker caches responses after first read
///    - Filesystem store tracks file position
///
/// 2. **Uniformity**: A single trait signature works for all stores. Stores that
///    don't mutate on read simply ignore the mutability—the compiler optimizes
///    this away.
///
/// 3. **No interior mutability tax**: Stores don't need `Mutex` or `RefCell`
///    internally just to satisfy the trait. This avoids runtime overhead and
///    potential deadlocks.
///
/// # Concurrent Access
///
/// For concurrent access to a store, wrap it explicitly:
///
/// ```rust
/// use std::sync::{Arc, Mutex};
///
/// let store = Arc::new(Mutex::new(MyStore::new()));
///
/// // In thread 1:
/// let mut guard = store.lock().unwrap();
/// guard.read(&path)?;
///
/// // In thread 2:
/// let mut guard = store.lock().unwrap();
/// guard.read(&other_path)?;
/// ```
///
/// This makes synchronization explicit at the usage site rather than hidden
/// in the trait design.
pub trait Reader: Send + Sync {
    /// Read data from the given path.
    ///
    /// Returns `Ok(Some(record))` if data exists at the path,
    /// `Ok(None)` if the path doesn't exist,
    /// or `Err` if an error occurred.
    fn read(&mut self, from: &Path) -> Result<Option<Record>, Error>;
}

/// Write data to a store at the given path.
///
/// See [`Reader`] for discussion of the `&mut self` requirement.
pub trait Writer: Send + Sync {
    /// Write data to the given path.
    ///
    /// Returns the path where data was written. This may differ from the
    /// input path—for example, the HTTP broker returns a handle path like
    /// `/outstanding/0` after queuing a request to the root path.
    fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error>;
}
```

### Step 2: Add architectural note to CLAUDE.md

**File:** `CLAUDE.md`

Add to "Architecture Decisions" section:

```markdown
4. **Mutable Reader/Writer traits**: Both `read()` and `write()` take `&mut self`.
   This is intentional—some stores (HTTP broker, filesystem) have state that
   changes on read. Using `&mut self` uniformly avoids the complexity of split
   traits or interior mutability. For concurrent access, wrap stores in
   `Arc<Mutex<_>>` explicitly.
```

### Step 3: Add design rationale document

**File:** `docs/design/mutability.md` (optional, if docs/ exists)

```markdown
# Why Reader::read takes &mut self

## The Question

Why does `Reader::read(&mut self, ...)` require mutable access when many stores
don't mutate on read?

## The Answer

Some stores DO mutate on read:

- **HttpBrokerStore**: Executes HTTP request on first read, caches response
- **FsStore**: Updates file position after reading
- **OverlayStore**: Delegates to child stores that may mutate

Rather than split into `ImmutableReader` and `MutableReader` traits, we use a
single `&mut self` signature. Stores that don't need mutation simply ignore it.

## Trade-offs

**Pros:**
- Simple, uniform API
- No interior mutability overhead
- Clear ownership semantics
- Blanket impls work naturally

**Cons:**
- Requires `&mut` even for pure reads
- Can't share a store across threads without explicit synchronization

## Concurrent Access Pattern

```rust
let store = Arc::new(Mutex::new(store));

// Each thread locks before access
let mut guard = store.lock().unwrap();
guard.read(&path)?;
```

This is explicit and avoids hidden synchronization costs.
```

## Files Changed

- `packages/core-store/src/lib.rs` - Add trait documentation
- `CLAUDE.md` - Add architecture note

## Complexity

Low - Documentation only, no code changes.
