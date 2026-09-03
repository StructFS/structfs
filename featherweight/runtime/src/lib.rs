//! # featherweight-runtime
//!
//! A strawman Isotope runtime (`isotope/spec/`): blocks are pico-processes
//! whose entire world is StructFS reads and writes.
//!
//! - **Blocks** run native Rust ([`NativeBlock`]) or wasm components
//!   ([`WasmBlock`]) on blocking threads, against a per-block
//!   [`Namespace`].
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
//! use featherweight_runtime::{AssemblyDef, Runtime, register_builtins};
//! use std::collections::HashMap;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let rt = tokio::runtime::Runtime::new()?;
//! let mut runtime = Runtime::with_handle(rt.handle().clone());
//! register_builtins(&mut runtime);
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

pub mod assembly;
pub mod block;
mod error;
pub mod iso;
pub mod namespace;
pub mod native;
pub mod protocol;
mod runtime;
pub mod spawn;
pub mod stdio;
pub mod wasm_block;

pub use assembly::{AssemblyDef, BlockDef, WireDef, WireTarget};
pub use block::{
    BlockCell, BlockEvent, BlockId, BlockState, FailurePolicy, ServerRequest, ShutdownMode,
};
pub use error::{Result, RuntimeError};
pub use iso::{IsoSurface, LogSink, StderrLog};
pub use namespace::{host_store, GrantStore, HostStore, Namespace, Target, WiringTable};
pub use native::{register_builtins, NativeBlock, NativeBlockFactory, ShellBlock};
pub use runtime::{AssemblyInstance, Runtime, StdioProvider};
pub use spawn::{ProcStore, SpawnProtocol};
pub use stdio::{HostStdio, NullStdio, ScriptedStdio, Stdio};
pub use wasm_block::WasmBlock;
