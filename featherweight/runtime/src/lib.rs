//! # featherweight-runtime
//!
//! A strawman Isotope runtime
//! ([the spec](https://github.com/StructFS/structfs/tree/main/isotope/spec)):
//! blocks are pico-processes
//! whose entire world is StructFS reads and writes.
//!
//! - **Blocks** run native Rust ([`NativeBlock`]) or core-binding wasm
//!   ([`CoreWasmBlock`]) against a per-block [`Namespace`]. Core Wasm
//!   runs on async tasks; native blocks use blocking threads. Other
//!   artifact kinds (e.g. WIT components via the `featherweight-component`
//!   adapter) register through [`RuntimeConfig::register_loader`].
//! - **Two supported ways to start guest code**: [`Runtime::instantiate`]
//!   runs an assembly (blocks wired into namespaces), and
//!   [`CoreWasmBlock::start_sync`] / [`CoreWasmBlock::start_async`] run one
//!   prepared core-wasm artifact over a host store you own and get back.
//! - **`/iso/`** ([`IsoSurface`]) is the syscall surface: identity,
//!   lifecycle, time, randomness, logging, and the server protocol.
//! - **The server protocol** ([`protocol`]) makes every block a store:
//!   operations routed to a block become `{op, path, data, respond_to}`
//!   requests read from `iso/server/requests`; the block's response write
//!   resolves the caller's parked operation.
//! - **Assemblies** ([`AssemblyDef`], [`Runtime::instantiate`]) compose
//!   blocks with capability wiring; nested assembly definitions
//!   instantiate recursively (the fractal property), the public block
//!   starts eagerly, and everything else starts lazily on first access.
//!
//! ## Example
//!
//! ```rust,no_run
//! use featherweight_runtime::{AssemblyDef, Runtime, RuntimeConfig, register_builtins};
//! use std::collections::HashMap;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let rt = tokio::runtime::Runtime::new()?;
//! let mut config = RuntimeConfig::new(rt.handle().clone());
//! register_builtins(&mut config);
//! let runtime = Runtime::new(config);
//!
//! let def = AssemblyDef::from_str(r#"
//! assembly: demo
//! blocks:
//!   kv: builtin:kv
//! public: kv
//! "#)?;
//! let assembly = runtime.instantiate(&def, HashMap::new(), ".".as_ref())?;
//!
//! rt.block_on(async {
//!     use structfs_core_store::{path, Value};
//!     assembly.write(path!("users/alice"), Value::from("hi")).await?;
//!     let value = assembly.read(path!("users/alice")).await?;
//!     assert_eq!(value, Some(Value::from("hi")));
//!     Ok::<_, structfs_core_store::Error>(())
//! })?;
//! # Ok(())
//! # }
//! ```

#[doc(hidden)]
pub mod adapter;
pub mod driver;
pub use driver::{DriverContext, DriverCounter, ExecutionMeter, ExecutionUsage};
pub mod execution;
pub use execution::ExecutionScope;
pub mod admission;
pub use admission::{CallBudget, CallBudgetSnapshot, CallLimits, CallMetrics, CallUsage};
pub mod assembly;
pub mod block;
pub mod core_wasm;
pub mod determinism;
mod error;
pub(crate) mod hash;
pub mod iso;
pub mod metering;
pub mod namespace;
pub mod native;
pub mod protocol;
mod runtime;
pub mod session;
pub mod spawn;
pub mod stdio;
pub mod transcript;
pub(crate) mod turnstile;

pub use assembly::{AssemblyDef, BlockDef, WireDef, WireTarget};
pub use block::{BlockId, BlockState, BlockView, FailurePolicy, ShutdownMode};
pub use core_wasm::{CoreWasmBlock, CoreWasmEngine, CoreWasmSession, NoOpStore};
pub use determinism::Determinism;
pub use error::{Result, RuntimeError};
pub use iso::{IsoSurface, LogSink, StderrLog};
pub use metering::Metering;
pub use namespace::{async_host_store, host_store, service_host_store, HostStore, Namespace};
pub use native::{register_builtins, NativeBlock, NativeBlockFactory, ShellBlock};
pub use protocol::ErrorKind;
pub use runtime::{
    ArtifactLoader, AssemblyInstance, AssemblyRequest, Runtime, RuntimeConfig, ShutdownReport,
    StdioProvider, WasmBlockDriver,
};
pub use session::SessionEntry;
pub use spawn::{ProcStore, SpawnProtocol};
pub use stdio::{HostStdio, NullStdio, ScriptedStdio, Stdio};
pub use transcript::{
    SeekPoint, TranscriptAnswer, TranscriptEntry, TranscriptMode, TranscriptProvider,
};

mod hosting;
pub use hosting::{
    ExecutionOutcome, ExecutionOwner, ExecutionPolicy, ExecutionStartError, GrowthFailure,
};
