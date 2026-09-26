//! # structfs-repl
//!
//! An interactive REPL for StructFS operations.
//!
//! This crate provides a command-line interface for reading and writing
//! to StructFS stores using JSON serialization.
//!
//! ## Architecture
//!
//! The REPL is split into platform-independent core and platform-specific host:
//!
//! - **`repl`**: The main REPL loop, interacts only through `IoHost` trait
//! - **`io`**: Types and traits for I/O abstraction
//! - **`host`**: Platform-specific implementations (terminal, future: Wasm)
//!
//! ## Features
//!
//! - Mount in-memory, JSON-file, HTTP, log and recording stores
//! - Read and write JSON data at any path, and list children with `ls`
//! - Tab completion for commands
//! - Syntax highlighting for JSON input
//! - Vi mode support (`--vi`/`--emacs`, else STRUCTFS_EDIT_MODE, EDITOR, or
//!   the inputrc's `set editing-mode`)
//! - Command history
//!
//! ## Usage
//!
//! ```bash
//! # Run the REPL
//! structfs
//!
//! # Inside the REPL:
//! > read /ctx/mounts
//! > write /ctx/mounts/data {"type": "memory"}
//! > write /data/users/1 {"name": "Alice", "email": "alice@example.com"}
//! > read /data/users/1
//! ```

pub mod command_table;
pub mod commands;
pub mod completer;
pub mod help_format;
pub mod help_store;
pub mod highlighter;
pub mod host;
pub mod io;
pub mod mounts;
pub mod recording_store;
pub mod repl;
pub mod repl_docs_store;
pub mod store_context;

// Re-exports
pub use host::{EditMode, TerminalHost};
pub use io::{ExitReason, IoError, IoHost, Output, PromptConfig, Signal};
pub use mounts::{CoreReplStoreFactory, MountConfig};
pub use repl::ReplCore;
pub use store_context::StoreContext;

/// Run the REPL with the terminal host until the user exits.
///
/// `edit_mode` overrides edit-mode detection (see
/// [`TerminalHost::with_edit_mode`]). This is the entry point for the CLI.
pub fn run(edit_mode: Option<EditMode>) -> Result<ExitReason, IoError> {
    let mut core = ReplCore::new();
    let mut host = TerminalHost::with_edit_mode(edit_mode)?;
    core.run(&mut host)
}
