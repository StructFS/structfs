//! Revisioned state protocol and, with `service`, the in-memory reference provider.
//! The protocol-only build is usable from core-Wasm guests.
//!
//! # Conventions
//!
//! **Constructors return `Self`**, matching `structfs-service` — with one
//! exception: [`State::shared`] is the only way to build a [`State`], and it
//! returns `Arc<State>`, because the state registers its own teardown and
//! retained-bytes reservation with its owner through a weak reference.
//!
//! **Stop verbs**: [`StateHandle::close`] is the only one here, and it is
//! `async` because releasing a handle is a write to the provider; there is
//! no `join` because cleanup finishes inside that write.
//!
//! **Isolation is per view.** Handles and faults belong to the view they were
//! opened through, plus the owner when the client is bound with `owned_by`.
//! Clients sharing one view without an owner share its handles by design —
//! the view is the grant — so hosts should call `State::view` once per block
//! or tenant. Another view's handle reads as `Fault::Closed`, and releasing
//! it is a silent no-op, exactly like an unknown id.
//!
//! **One page envelope.** [`SnapshotPage`], [`ChangePage`] and
//! `structfs_service::TailRead` all carry `items` (this page's entries),
//! `next` (the cursor to pass to the following read) and `done` (no further
//! entries will ever arrive). Only the cursor type differs: a `u64` offset
//! into a pinned snapshot, a [`Token`] for a change stream.
//!
//! **`#[non_exhaustive]`.** Every wire type here can gain fields. Build the
//! limit types from `default()` plus their `with_*` setters, and build
//! [`Token`] with [`Token::new`].
use serde::{Deserialize, Serialize};
use structfs_core_store::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Token {
    pub epoch: String,
    pub revision: u64,
}
impl Token {
    pub fn new(epoch: impl Into<String>, revision: u64) -> Self {
        Self {
            epoch: epoch.into(),
            revision,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Mutation {
    Set { path: String, value: Value },
    Delete { path: String },
}
impl Mutation {
    pub fn path(&self) -> &str {
        match self {
            Self::Set { path, .. } | Self::Delete { path } => path,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReadLimits {
    pub page_bytes: usize,
    pub page_items: usize,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            page_bytes: 65536,
            page_items: 128,
        }
    }
}
impl ReadLimits {
    /// Largest encoded reply this reader will accept for one page.
    pub fn with_page_bytes(mut self, bytes: usize) -> Self {
        self.page_bytes = bytes;
        self
    }
    /// Most entries this reader will accept in one page.
    pub fn with_page_items(mut self, items: usize) -> Self {
        self.page_items = items;
        self
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Command {
    Batch {
        #[serde(default)]
        expected: Option<Token>,
        mutations: Vec<Mutation>,
    },
    Snapshot {
        prefix: String,
        limits: ReadLimits,
    },
    Observe {
        prefix: String,
        limits: ReadLimits,
    },
    Watch {
        prefix: String,
        after: Token,
        limits: ReadLimits,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Request {
    pub version: u32,
    #[serde(flatten)]
    pub command: Command,
}
impl Request {
    pub fn new(command: Command) -> Self {
        Self {
            version: 1,
            command,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Descriptor {
    pub token: Token,
    pub snapshot: bool,
    pub watch: bool,
    pub committed: bool,
}
/// Preorder nodes: containers carry empty Map/Array skeletons, leaves carry values.
/// Paths are relative to the requested snapshot prefix. An empty snapshot is missing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Node {
    pub path: Vec<String>,
    pub value: Value,
}
impl Node {
    pub fn new(path: Vec<String>, value: Value) -> Self {
        Self { path, value }
    }
}
/// One page of a pinned snapshot. See the crate-level "one page envelope" note.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SnapshotPage {
    pub token: Token,
    pub items: Vec<Node>,
    pub next: u64,
    pub done: bool,
}
/// Authoritative commit invalidations, not a value diff or an operation trace.
/// Paths conservatively include the parents of mutations (including array shifts).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Change {
    pub token: Token,
    pub paths: Vec<String>,
}
/// One page of a change stream. See the crate-level "one page envelope" note.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ChangePage {
    pub items: Vec<Change>,
    pub next: Token,
    pub done: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Fault {
    Invalid { message: String },
    Conflict { current: Token },
    EpochMismatch { current: Token },
    CursorExpired { earliest: Token },
    ResourceLimit { message: String },
    Closed,
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid { message } => write!(f, "invalid request: {message}"),
            Self::Conflict { current } => write!(
                f,
                "expected revision did not match; state is at revision {}",
                current.revision
            ),
            Self::EpochMismatch { current } => write!(
                f,
                "token belongs to another state epoch; current epoch is {}",
                current.epoch
            ),
            Self::CursorExpired { earliest } => write!(
                f,
                "cursor fell behind retained history; earliest available revision is {}",
                earliest.revision
            ),
            Self::ResourceLimit { message } => write!(f, "state limit exceeded: {message}"),
            Self::Closed => write!(f, "state handle is closed"),
        }
    }
}
impl std::error::Error for Fault {}
/// State errors travel as values, so even the diagnostic-only core-Wasm ABI
/// preserves typed conflict/expiry information. Transport failures remain separate.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Reply<T> {
    Ok(T),
    Error(Fault),
}
impl<T> Reply<T> {
    pub fn into_result(self) -> Result<T, Fault> {
        match self {
            Self::Ok(v) => Ok(v),
            Self::Error(e) => Err(e),
        }
    }
}
#[cfg(feature = "service")]
mod faults;
#[cfg(feature = "service")]
mod limits;
#[cfg(feature = "service")]
pub use limits::StateLimits;
#[cfg(feature = "service")]
mod paging;
#[cfg(feature = "service")]
mod provider;
#[cfg(feature = "service")]
pub use provider::State;
#[cfg(feature = "service")]
mod client;
#[cfg(feature = "service")]
mod view;
#[cfg(feature = "service")]
pub use client::{ClientError, StateClient, StateHandle};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Capabilities {
    pub version: u32,
    pub durability: String,
    pub max_change_bytes: usize,
    pub min_page_bytes: usize,
    pub max_page_bytes: usize,
    pub max_page_items: usize,
}

/// An immutable, client-owned projection for synchronous renderers. Fetching
/// and applying effects are separate from reading this pinned view.
#[non_exhaustive]
pub struct Projection {
    pub token: Token,
    root: Option<Value>,
}
impl Projection {
    pub fn from_nodes(token: Token, nodes: Vec<Node>) -> Result<Self, Fault> {
        let mut root = None;
        for node in nodes {
            if node.path.len() > 64 {
                return Err(Fault::ResourceLimit {
                    message: "projection depth".into(),
                });
            }
            if node.path.is_empty() {
                if root.is_some() {
                    return Err(Fault::Invalid {
                        message: "duplicate snapshot root".into(),
                    });
                }
                root = Some(node.value);
                continue;
            }
            let mut parent = root.as_mut().ok_or_else(|| Fault::Invalid {
                message: "missing snapshot root".into(),
            })?;
            for component in &node.path[..node.path.len() - 1] {
                parent = match parent {
                    Value::Map(m) => m.get_mut(component),
                    Value::Array(a) => component.parse::<usize>().ok().and_then(|i| a.get_mut(i)),
                    _ => None,
                }
                .ok_or_else(|| Fault::Invalid {
                    message: "invalid snapshot parent".into(),
                })?;
            }
            let key = node.path.last().unwrap();
            match parent {
                Value::Map(m) => {
                    if m.insert(key.clone(), node.value).is_some() {
                        return Err(Fault::Invalid {
                            message: "duplicate snapshot node".into(),
                        });
                    }
                }
                Value::Array(a) if key.parse::<usize>().ok() == Some(a.len()) => a.push(node.value),
                _ => {
                    return Err(Fault::Invalid {
                        message: "invalid snapshot node".into(),
                    })
                }
            }
        }
        Ok(Self { token, root })
    }
    pub fn root(&self) -> Option<&Value> {
        self.root.as_ref()
    }
}
impl structfs_core_store::Reader for Projection {
    fn read(
        &mut self,
        p: &structfs_core_store::Path,
    ) -> Result<Option<structfs_core_store::Record>, structfs_core_store::Error> {
        Ok(self
            .root
            .as_ref()
            .and_then(|v| v.get(p))
            .cloned()
            .map(structfs_core_store::Record::parsed))
    }
}
