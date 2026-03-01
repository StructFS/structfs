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
//! use structfs::{Reader, Writer, Record, Path, Value, path};
//!
//! fn greet(store: &mut dyn Reader) -> Result<Option<Record>, structfs::Error> {
//!     store.read(&path!("greeting"))
//! }
//! ```
//!
//! ## Features
//!
//! The default feature set includes only the core interface. Store implementations
//! are opt-in:
//!
//! | Feature | What it adds |
//! |---------|-------------|
//! | `serde` | `TypedReader`, `TypedWriter`, `JsonCodec`, value conversions |
//! | `json`  | `InMemoryStore` (implies `serde`) |
//! | `http`  | `HttpBrokerStore`, `HttpClientStore` (implies `serde`) |
//! | `sys`   | `SysStore` — env, time, random, proc, fs |
//! | `async` | Async trait variants |
//! | `full`  | All of the above |

// ── Core: always available ──────────────────────────────────────────

pub use structfs_core_store::{
    Bytes, Codec, CodecOperation, CoreToLL, Error, Format, LLError, LLPath, LLReader, LLStore,
    LLToCore, LLWriter, LazyRecord, NoCodec, Path, PathError, PathTrie, Reader, Record, Reference,
    Store, TypeDescriptor, TypeInfo, Value, Writer,
};

/// Path macro for constructing validated [`Path`] values from string literals.
///
/// ```rust
/// use structfs::path;
/// let p = path!("users/123");
/// ```
pub use structfs_core_store::path;

/// Mount infrastructure: config-driven store creation and path routing.
pub use structfs_core_store::mount_store;
pub use structfs_core_store::mount_store::{MountConfig, MountInfo, MountStore, StoreFactory};

/// Overlay infrastructure: composing multiple stores into a unified tree.
pub use structfs_core_store::overlay_store;
pub use structfs_core_store::overlay_store::{
    OnlyReadable, OnlyWritable, OverlayStore, RedirectMode, RouteTarget, StoreBox, SubStoreView,
};

/// Path-keyed prefix trie.
pub use structfs_core_store::path_trie;

// ── Async core (feature = "async") ──────────────────────────────────

#[cfg(feature = "async")]
pub use structfs_core_store::{
    AsyncCoreToLL, AsyncLLReader, AsyncLLStore, AsyncLLToCore, AsyncLLWriter, AsyncReader,
    AsyncStore, AsyncWriter, SyncToAsync, SyncToAsyncLL,
};

// ── Serde integration (feature = "serde") ───────────────────────────

#[cfg(feature = "serde")]
pub mod serde {
    //! Serde integration: typed access and codecs.
    pub use structfs_serde_store::{
        from_value, json_to_value, to_value, value_to_json, JsonCodec, MultiCodec, TypedReader,
        TypedWriter,
    };

    #[cfg(feature = "async")]
    pub use structfs_serde_store::{AsyncTypedReader, AsyncTypedWriter};
}

#[cfg(feature = "serde")]
pub use structfs_serde_store::{
    from_value, json_to_value, to_value, value_to_json, JsonCodec, MultiCodec, TypedReader,
    TypedWriter,
};

#[cfg(all(feature = "serde", feature = "async"))]
pub use structfs_serde_store::{AsyncTypedReader, AsyncTypedWriter};

// ── JSON store (feature = "json") ───────────────────────────────────

#[cfg(feature = "json")]
pub mod json {
    //! JSON-based in-memory store.
    pub use structfs_json_store::{in_memory, value_utils, InMemoryStore};
}

#[cfg(feature = "json")]
pub use structfs_json_store::InMemoryStore;

// ── HTTP stores (feature = "http") ──────────────────────────────────

#[cfg(feature = "http")]
pub mod http {
    //! HTTP client and broker stores.
    pub use structfs_http::{
        error, executor, handle, types, AsyncHttpBrokerStore, Error, HttpBrokerStore,
        HttpClientStore, HttpExecutor, HttpRequest, HttpResponse, Method, RequestState,
        RequestStatus, ReqwestExecutor,
    };
}

#[cfg(feature = "http")]
pub use structfs_http::{
    AsyncHttpBrokerStore, HttpBrokerStore, HttpClientStore, HttpExecutor, HttpRequest,
    HttpResponse, Method,
};

// ── System store (feature = "sys") ──────────────────────────────────

#[cfg(feature = "sys")]
pub mod sys {
    //! OS primitives: env, time, random, proc, fs.
    pub use structfs_sys::{
        DocsStore, EnvStore, FsStore, OpenMode, ProcStore, RandomStore, SysStore, TimeStore,
    };
}

#[cfg(feature = "sys")]
pub use structfs_sys::SysStore;
