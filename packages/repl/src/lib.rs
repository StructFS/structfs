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
//! - Connect to local JSON stores or remote HTTP endpoints
//! - Read and write JSON data at any path
//! - Tab completion for commands
//! - Syntax highlighting for JSON input
//! - Vi mode support (detected from EDITOR, .inputrc, or STRUCTFS_EDIT_MODE)
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

pub mod commands;
pub mod completer;
pub mod help_store;
pub mod highlighter;
pub mod host;
pub mod io;
pub mod repl;
pub mod repl_docs_store;
pub mod store_context;

// Re-exports
pub use host::TerminalHost;
pub use io::{ExitReason, IoHost, Output, PromptConfig, Signal};
pub use repl::ReplCore;
pub use store_context::StoreContext;

/// Run the REPL with the terminal host.
///
/// This is the main entry point for the CLI application.
pub fn run() -> std::io::Result<()> {
    let mut core = ReplCore::new();
    let mut host = TerminalHost::new()?;

    match core.run(&mut host) {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
