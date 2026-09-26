//! # StructFS
//!
//! A uniform interface for accessing data through read/write operations on paths.
//!
//! StructFS is two verbs and a noun: **read**, **write**, **path**. All data access —
//! including mount management, HTTP requests, and configuration — happens through the
//! same read/write interface on paths.
//!
//! ## Quick Start
//!
//! ```rust
//! use structfs::{path, MemoryStore, Reader, Record, Value, Writer};
//!
//! let mut store = MemoryStore::new();
//! store.write(&path!("greeting"), Record::parsed(Value::from("hello")))?;
//! assert!(store.read(&path!("greeting"))?.is_some());
//! # Ok::<(), structfs::Error>(())
//! ```
//!
//! ## Layout: one path per item
//!
//! Every item has exactly one path in this crate:
//!
//! - **The root** holds the core vocabulary of `structfs-core-store`: the
//!   [`Reader`]/[`Writer`]/[`Store`] traits, [`Path`], [`PathComponent`],
//!   [`path!`], [`Value`], [`Record`], [`Format`], [`Error`], [`MemoryStore`],
//!   the combinators ([`ReadOnly`], [`Cascade`], [`Shared`], [`Rooted`],
//!   [`Masked`]), mounting ([`MountStore`], [`OverlayStore`]) and the
//!   [`conformance`] checks. With the `async` feature the root also holds the
//!   async and detached trait families (`AsyncReader`, `DetachedReader`,
//!   `SharedReader`, …). [`Bytes`] is the `bytes` crate's type, re-exported
//!   because [`Record::raw`] takes it.
//! - **Supporting core modules** are always present: [`ll`] (the byte
//!   layer), [`pattern`] (component-wise path matching) and [`path_serde`]
//!   (serde `with` helpers for [`Path`] fields).
//! - **Every feature-gated area is a module named after its feature**, and its
//!   items appear only there: `structfs::typed::JsonCodec`, never
//!   `structfs::JsonCodec`.
//! - `typed`, `persist`, `net`, `os` and `handles` list their items one by
//!   one, because those crates also re-export core types for their own
//!   callers. `service`, `state` and `profiles` are flat single-module crates
//!   that re-export nothing from other crates, so each is aliased whole
//!   (`structfs::service` *is* `structfs_service`) without creating a second
//!   path to anything.
//!
//! Module names are chosen so that `use structfs::*;` never shadows a
//! well-known crate: there is no `structfs::serde`, `structfs::http`,
//! `structfs::json` or `structfs::sys`, so glob imports sit beside `serde`,
//! `http` and friends without ambiguity:
//!
//! ```rust
//! use structfs::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct Greeting {
//!     text: String,
//! }
//!
//! let mut store = MemoryStore::new();
//! store.write(&path!("greeting"), Record::parsed(Value::from("hello")))?;
//! # let _ = Greeting { text: String::new() };
//! # Ok::<(), Error>(())
//! ```
//!
//! ## Features
//!
//! Default features expose only the core interface. Everything else is opt-in:
//!
//! | Feature | Module | What it adds | Implies |
//! |---------|--------|--------------|---------|
//! | `async` | root | Async and detached trait families; async helpers in `typed` | |
//! | `typed` | `typed` (`structfs-serde-store`) | Serde-typed access, codecs, [`Value`] conversions, `Limits` | |
//! | `persist` | `persist` (`structfs-json-store`) | `BackedStore`, `LogStore` and their file/memory backings | `typed` |
//! | `net` | `net` (`structfs-http`) | Portable HTTP schema: request/response/status types, SSE framing, errors | |
//! | `net-blocking` | `net` | Native blocking HTTP stores and `BlockingReqwestExecutor` | `net` |
//! | `net-streaming` | `net::streaming` | Native async streaming executor | `net`, `async` |
//! | `os` | `os` (`structfs-sys`) | `SysStore` — env, time, random, proc, fs, docs | |
//! | `handles` | `handles` (`structfs-handles`) | Handle stores, gates, cancellation, duplex streams | `async` |
//! | `service` | `service` (`structfs-service`) | Shared async routing and owned providers | `handles`, `async` |
//! | `state` | `state` (`structfs-state`) | Revisioned state and bounded observation | `service` |
//! | `profiles` | `profiles` (`structfs-profiles`) | Capability profiles and headless interactive sessions | `service` |
//! | `full` | all | Every feature above | |
//!
//! The core, `async`, `typed`, `net` and `handles` are browser-shareable
//! (checked for `wasm32-unknown-unknown`); `net-blocking` and `net-streaming`
//! pull in a native HTTP client. See `docs/platforms.md` in the repository.

// ── Core vocabulary: always available ───────────────────────────────

pub use structfs_core_store::{
    Bytes, Cascade, ChildNames, ChildPage, Codec, CodecDiagnostic, CodecErrorKind, CodecOperation,
    Error, Format, LazyRecord, Masked, MemoryStore, NoCodec, Path, PathComponent, PathError,
    ReadOnly, Reader, Record, Reference, Rooted, Shared, Store, TypeDescriptor, TypeInfo, Value,
    Writer,
};

pub use structfs_core_store::mount_store::{MountStore, StoreFactory};
pub use structfs_core_store::overlay_store::{OverlayStore, RedirectMode, StoreBox};
pub use structfs_core_store::path_trie::{PathTrie, PathTrieIter};

/// Construct a validated [`Path`] from string literals (checked at compile
/// time) and [`PathComponent`] expressions. Expansion is hygienic through
/// this facade and through renamed dependencies.
///
/// ```rust
/// use structfs::path;
/// let p = path!("users/123");
/// assert_eq!(p.len(), 2);
/// ```
pub use structfs_core_store::path;

pub use structfs_core_store::{conformance, path_serde};

#[cfg(feature = "async")]
pub use structfs_core_store::{
    AsyncReader, AsyncStore, AsyncWriter, DetachedFuture, DetachedReader, DetachedShared,
    DetachedStore, DetachedWriter, SharedReader, SharedWriter, SyncToAsync,
};

pub mod ll {
    //! The byte layer (`structfs-ll-store`): paths of raw byte components and
    //! stores that move opaque bytes, plus [`CoreToLL`], which serves a core
    //! [`Store`](crate::Store) through that interface.
    //!
    //! Most code never needs this module; it exists for transports and
    //! embedders that speak bytes.
    pub use structfs_core_store::bridge_protocol as protocol;
    pub use structfs_core_store::{CoreToLL, LLError, LLPath, LLReader, LLStore, LLWriter};

    #[cfg(feature = "async")]
    pub use structfs_core_store::{AsyncLLReader, AsyncLLStore, AsyncLLWriter, SyncToAsyncLL};
}

pub mod pattern {
    //! Component-wise path matching: [`PathPattern`] (used by
    //! [`Masked`](crate::Masked)) and the [`matches_prefix_suffix`] primitive
    //! it is built on.
    pub use structfs_core_store::{matches_prefix_suffix, PathPattern};
}

// ── Feature-gated areas: one module each ────────────────────────────

#[cfg(feature = "typed")]
pub mod typed {
    //! Typed access through serde, and the wire codecs (`structfs-serde-store`).
    //!
    //! Named `typed` rather than `serde` so that `use structfs::*` does not
    //! shadow the `serde` crate. [`CodecProfile`] is the codec wire profile —
    //! unrelated to the capability contracts in `structfs::profiles`.
    pub use structfs_serde_store::{
        from_value, from_value_with_limits, json_to_value, to_value, to_value_with_limits,
        transcode, value_to_json, CborCodec, CodecProfile, ExplicitOption, FlexbuffersCodec,
        JsonCodec, Limits, MultiCodec, TypedReader, TypedWriter, ValueCodec, ValueJsonCodec,
    };

    #[cfg(feature = "async")]
    pub use structfs_serde_store::{
        AsyncTypedReader, AsyncTypedWriter, DetachedTypedReader, DetachedTypedWriter,
    };
}

#[cfg(feature = "persist")]
pub mod persist {
    //! Durable stores (`structfs-json-store`): [`BackedStore`] rewrites a
    //! whole snapshot per write, [`LogStore`] appends entries to a log.
    //!
    //! Named `persist` for what it provides — durability — rather than the
    //! on-disk encoding, and so that it does not shadow a `json` crate.
    pub use structfs_json_store::{
        AppendBacking, BackedStore, Backing, Durability, JsonFileBacking, JsonlFileBacking,
        LogStore, MemoryAppendBacking,
    };
}

#[cfg(feature = "net")]
pub mod net {
    //! Network stores (`structfs-http`).
    //!
    //! Named `net` rather than `http` so that `use structfs::*` does not
    //! shadow the `http` crate.
    //!
    //! - `net`: the portable schema — [`HttpRequest`], [`HttpResponse`],
    //!   [`Method`], [`RequestState`]/[`RequestStatus`], [`Error`] and
    //!   [`sse`] framing. No native HTTP client; builds for `wasm32`.
    //! - `net-blocking`: `HttpBrokerStore`, `BackgroundHttpBrokerStore`,
    //!   `HttpClientStore` and the `BlockingHttpExecutor` seam with its
    //!   `BlockingReqwestExecutor` implementation.
    //! - `net-streaming`: the `streaming` module's async executor.
    pub use structfs_http::sse;
    pub use structfs_http::{
        Error, HttpRequest, HttpResponse, Method, RequestState, RequestStatus,
    };

    #[cfg(feature = "net-blocking")]
    pub use structfs_http::{
        BackgroundHttpBrokerStore, BlockingHttpExecutor, BlockingReqwestExecutor, HttpBrokerStore,
        HttpClientStore,
    };

    #[cfg(feature = "net-streaming")]
    pub use structfs_http::streaming;
}

#[cfg(feature = "os")]
pub mod os {
    //! Operating-system primitives (`structfs-sys`): env, time, random, proc,
    //! fs and docs, composed by [`SysStore`].
    //!
    //! Named `os` rather than `sys` so that `use structfs::*` does not shadow
    //! a `sys` crate, and because it matches what the stores expose.
    pub use structfs_sys::{
        ContentEncoding, DocsStore, EnvStore, FsStore, OpenMode, ProcStore, RandomStore, SysStore,
        TimeStore, DEFAULT_MAX_HANDLES, DEFAULT_MAX_READ_LEN, DEFAULT_MAX_RESULTS,
        MAX_RANDOM_BYTES, MAX_SLEEP,
    };
}

#[cfg(feature = "handles")]
pub mod handles {
    //! Handle-store and streaming primitives (`structfs-handles`):
    //! `outstanding/{id}` scaffolding, lost-wakeup-proof gates, cancellation
    //! and bounded duplex streams.
    pub use structfs_handles::conformance;
    pub use structfs_handles::{
        CancelToken, Cancelled, DuplexStream, Gate, HandleCx, HandleProtocol, HandleStore,
        StreamReadiness, StreamStore,
    };
}

/// Native async service routing and provider adapters (`structfs-service`).
#[cfg(feature = "service")]
pub use structfs_service as service;

/// Revisioned state, snapshots, and bounded observation (`structfs-state`).
#[cfg(feature = "state")]
pub use structfs_state as state;

/// Versioned capability profiles and headless interactive sessions
/// (`structfs-profiles`).
#[cfg(feature = "profiles")]
pub use structfs_profiles as profiles;
