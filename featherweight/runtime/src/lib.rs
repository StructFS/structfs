//! # Featherweight Runtime
//!
//! Featherweight is the runtime layer for Isotope, a virtual operating system
//! built on StructFS principles. In Isotope, everything is a store - processes
//! communicate exclusively through read/write operations on paths.
//!
//! ## Core Concepts
//!
//! ### Blocks
//!
//! A **Block** is the fundamental unit of execution in Isotope - analogous to
//! a process in traditional operating systems, but much lighter weight. Each
//! Block:
//!
//! - Runs in a WASM sandbox (via Wasmtime) for memory isolation
//! - Has a single root store that represents its entire view of the world
//! - Communicates with other Blocks only through StructFS read/write operations
//! - Can export stores for other Blocks to consume
//!
//! Blocks are "pico-processes" - they're designed to be extremely lightweight,
//! allowing thousands to run concurrently.
//!
//! ### The Root Store
//!
//! Each Block sees only its **root store**. Everything the Block needs -
//! configuration, services, inter-Block communication - is mounted into this
//! root. The Block doesn't know or care whether a path leads to a local
//! in-memory store, a remote service, or another Block's export.
//!
//! This is the key to Isotope's simplicity: there's no distinction between
//! "local" and "remote" or "in-process" and "out-of-process". Everything is
//! just paths.
//!
//! ### Store Exports
//!
//! Blocks can **export** stores for other Blocks to consume. For example:
//!
//! - A logging Block exports a store at "log" that accepts log messages
//! - A database Block exports stores for each table
//! - An HTTP proxy Block exports a store that forwards requests
//!
//! The Runtime coordinates these exports, allowing Block A's export to be
//! mounted into Block B's root store.
//!
//! ## Example: Two Blocks Communicating
//!
//! ```ignore
//! use featherweight_runtime::{Block, BlockContext, Runtime, RuntimeConfig};
//! use structfs_core_store::{path, Reader, Writer, Record, Value};
//!
//! // A simple greeting service Block
//! struct GreetingService;
//!
//! #[async_trait::async_trait]
//! impl Block for GreetingService {
//!     async fn run<S: Reader + Writer + Send + 'static>(
//!         &mut self,
//!         mut ctx: BlockContext<S>,
//!     ) -> featherweight_runtime::Result<()> {
//!         // Read requests from /requests, write responses to /responses
//!         loop {
//!             if let Some(record) = ctx.root.read(&path!("requests"))? {
//!                 let name = record.into_value(&NoCodec)?;
//!                 if let Value::String(name) = name {
//!                     let greeting = format!("Hello, {}!", name);
//!                     ctx.root.write(
//!                         &path!("responses"),
//!                         Record::parsed(Value::String(greeting.into()))
//!                     )?;
//!                 }
//!             }
//!         }
//!     }
//! }
//!
//! // A client Block that uses the greeting service
//! struct Client;
//!
//! #[async_trait::async_trait]
//! impl Block for Client {
//!     async fn run<S: Reader + Writer + Send + 'static>(
//!         &mut self,
//!         mut ctx: BlockContext<S>,
//!     ) -> featherweight_runtime::Result<()> {
//!         // Write name to greeting service
//!         ctx.root.write(
//!             &path!("services/greeter/requests"),
//!             Record::parsed(Value::String("World".into()))
//!         )?;
//!
//!         // Read response
//!         let response = ctx.root.read(&path!("services/greeter/responses"))?;
//!         // response contains "Hello, World!"
//!         Ok(())
//!     }
//! }
//! ```
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Runtime                                  │
//! │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
//! │  │   Block A    │  │   Block B    │  │   Block C    │   ...    │
//! │  │  (WASM)      │  │  (WASM)      │  │  (WASM)      │          │
//! │  │              │  │              │  │              │          │
//! │  │  root_store  │  │  root_store  │  │  root_store  │          │
//! │  │   ├ config   │  │   ├ config   │  │   ├ config   │          │
//! │  │   ├ services/│  │   ├ services/│  │   ├ input    │          │
//! │  │   │  └ db ───┼──┼───┘         │  │   └ output   │          │
//! │  │   └ exports/ │  │              │  │              │          │
//! │  │      └ db    │  │              │  │              │          │
//! │  └──────────────┘  └──────────────┘  └──────────────┘          │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Design Philosophy
//!
//! ### Everything is a Store
//!
//! Following StructFS principles, Featherweight treats everything as a store:
//! - Process state is a store
//! - Inter-process communication is stores writing to stores
//! - System services are stores mounted at well-known paths
//!
//! ### Location Transparency
//!
//! Blocks don't know where data comes from. A path might lead to:
//! - An in-memory value
//! - Another Block's export
//! - A remote service
//! - A file on disk
//!
//! This enables migration, scaling, and testing without changing Block code.
//!
//! ### Capability-Based Security
//!
//! Blocks can only access what's in their root store. To give a Block access
//! to a resource, you mount it. This makes security boundaries explicit and
//! auditable.
//!
//! ## Strawman Implementation
//!
//! This initial implementation is a strawman - it demonstrates the concepts
//! without full WASM integration. Key simplifications:
//!
//! - Blocks run as native Rust async tasks, not WASM
//! - No memory isolation between Blocks
//! - Synchronous store access (future: async stores)
//!
//! These limitations will be addressed as the implementation matures.

pub mod block;
pub mod channel;
pub mod error;
pub mod runtime;

pub use block::{
    Block, BlockContext, BlockHandle, BlockId, BlockState, ErasedStore, ExportedStore,
};
pub use channel::ChannelStore;
pub use error::{Result, RuntimeError};
pub use runtime::{Runtime, RuntimeConfig, SharedStoreAdapter};
