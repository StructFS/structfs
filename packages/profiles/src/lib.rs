//! Independently versioned capability contracts. Discovery never invokes effects.
use serde::{Deserialize, Serialize};
pub use structfs_state::Token;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
pub enum Implementation {
    Reference,
    Compatibility,
    FixtureOnly,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub profile: Profile,
    pub version: u32,
    pub implementation: Implementation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Input {
    Key { text: String },
    Paste { text: String },
    Resize { columns: u32, rows: u32 },
    Mouse { x: u32, y: u32, button: u8 },
    Close,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputEnvelope {
    pub version: u32,
    pub session: String,
    pub sequence: u64,
    pub input: Input,
}
impl InputEnvelope {
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
pub struct SessionStatus {
    pub session: String,
    pub accepted: u64,
    pub processed: u64,
    pub rendered: Option<Token>,
    pub closed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Pending,
    Running,
    Completed,
    Failed,
}
/// Cancellation is a request, not a terminal phase or proof of rollback.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationStatus {
    pub operation: String,
    pub phase: Phase,
    pub cancel_requested: bool,
    pub joined: bool,
    pub result_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    Memory,
    FileSynced,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitAck {
    pub token: Token,
    pub persisted: bool,
    pub durability: Durability,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub version: u32,
    pub operation: String,
    pub approved: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRequest {
    pub version: u32,
    pub operation: String,
    pub program: String,
    pub args: Vec<String>,
    pub environment_grant: String,
    pub workspace_grant: String,
}
#[cfg(feature = "host")]
mod host;
#[cfg(feature = "host")]
pub use host::{HeadlessHost, Profiled, Session};
#[cfg(feature = "host")]
mod operation;
#[cfg(feature = "host")]
pub use operation::OperationHandle;
