//! Optional, independently versioned store contracts above StructFS core.
//! Stores implement their semantics; the runtime does not supply them.
//! Profile discovery never invokes effects.
//!
//! # Conventions
//!
//! **Constructors return `Self`**, matching `structfs-service`: wrap a
//! [`Session`] or [`OperationHandle`] in `Arc` yourself to mount it.
//!
//! **Stop verbs**: [`Session`] and [`OperationHandle`] have `close()`, which
//! requests cleanup without blocking, and `join(timeout)`, which waits for it
//! and returns the owner's report.
//! [`OperationHandle::cancel`] is deliberately *not* a stop verb — it asks the
//! running work to stop while keeping the result slot.
//!
//! **`#[non_exhaustive]`.** Every schema type here is a wire form that can
//! gain variants or fields; match with a wildcard arm.
use serde::{Deserialize, Serialize};
use structfs_state::Token;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Profile {
    #[serde(rename = "structfs.state")]
    State,
    #[serde(rename = "structfs.operation")]
    Operation,
    #[serde(rename = "structfs.binary_stream")]
    BinaryStream,
    #[serde(rename = "structfs.interactive")]
    Interactive,
    #[serde(rename = "structfs.configuration")]
    Configuration,
    #[serde(rename = "structfs.process")]
    Process,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Implementation {
    Reference,
    Compatibility,
    FixtureOnly,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Declaration {
    pub profile: Profile,
    pub version: u32,
    pub implementation: Implementation,
}
impl Declaration {
    /// Declare `profile` at version 1 with the given implementation tier.
    pub fn new(profile: Profile, implementation: Implementation) -> Self {
        Self {
            profile,
            version: 1,
            implementation,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Input {
    Key { text: String },
    Paste { text: String },
    Resize { columns: u32, rows: u32 },
    Mouse { x: u32, y: u32, button: u8 },
    Close {},
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct InputEnvelope {
    pub version: u32,
    pub session: String,
    pub sequence: u64,
    pub input: Input,
}
impl InputEnvelope {
    /// Version 1 envelope for `session` at `sequence`.
    pub fn new(session: impl Into<String>, sequence: u64, input: Input) -> Self {
        Self {
            version: 1,
            session: session.into(),
            sequence,
            input,
        }
    }
    /// Logical retained input weight; identical in Rust and the browser reference.
    pub fn weight(&self) -> Option<usize> {
        64usize
            .checked_add(self.session.len())?
            .checked_add(match &self.input {
                Input::Key { text } | Input::Paste { text } => text.len(),
                _ => 0,
            })
    }
    pub fn validate(&self, session: &str, last: u64) -> Result<(), &'static str> {
        if self.version != 1 {
            return Err("unsupported input version");
        }
        if self.session != session {
            return Err("wrong session");
        }
        if last.checked_add(1) != Some(self.sequence) {
            return Err("input sequence is not next");
        }
        if matches!(
            self.input,
            Input::Resize { columns: 0, .. } | Input::Resize { rows: 0, .. }
        ) {
            return Err("zero-sized presentation");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionStatus {
    pub session: String,
    pub accepted: u64,
    pub processed: u64,
    pub rendered: Option<Token>,
    pub closed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Phase {
    Pending,
    Running,
    Completed,
    Failed,
}
/// Cancellation is a request, not a terminal phase or proof of rollback.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OperationStatus {
    pub operation: String,
    pub phase: Phase,
    pub cancel_requested: bool,
    pub joined: bool,
    pub result_bytes: usize,
}
/// Levels in the optional configuration-store acknowledgment convention.
/// This is not a universal taxonomy of store durability guarantees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Durability {
    Memory,
    FileSynced,
}
/// A store-produced acknowledgment for this optional profile, not a core write
/// result or runtime persistence mechanism. Other stores may acknowledge durable
/// writes without this payload or a separate save operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CommitAck {
    pub token: Token,
    pub persisted: bool,
    pub durability: Durability,
}
impl CommitAck {
    /// An acknowledgment whose `persisted` flag matches its durability level.
    pub fn new(token: Token, durability: Durability) -> Self {
        Self {
            token,
            persisted: matches!(durability, Durability::FileSynced),
            durability,
        }
    }
    /// Check this schema's field consistency; does not verify persistence.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.persisted != matches!(self.durability, Durability::FileSynced) {
            return Err("inconsistent persistence acknowledgment");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Approval {
    pub version: u32,
    pub operation: String,
    pub approved: bool,
}
impl Approval {
    /// A version 1 approval decision for `operation`.
    pub fn new(operation: impl Into<String>, approved: bool) -> Self {
        Self {
            version: 1,
            operation: operation.into(),
            approved,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProcessRequest {
    pub version: u32,
    pub operation: String,
    pub program: String,
    pub args: Vec<String>,
    pub environment_grant: String,
    pub workspace_grant: String,
}
impl ProcessRequest {
    /// A version 1 request to run `program` with no arguments under the named
    /// grants. Set `args` afterwards when the program takes any.
    pub fn new(
        operation: impl Into<String>,
        program: impl Into<String>,
        environment_grant: impl Into<String>,
        workspace_grant: impl Into<String>,
    ) -> Self {
        Self {
            version: 1,
            operation: operation.into(),
            program: program.into(),
            args: Vec::new(),
            environment_grant: environment_grant.into(),
            workspace_grant: workspace_grant.into(),
        }
    }
}
#[cfg(feature = "host")]
mod host;
#[cfg(feature = "host")]
pub use host::{HeadlessHost, Profiled, Session};
#[cfg(feature = "host")]
mod operation;
#[cfg(feature = "host")]
pub use operation::OperationHandle;
