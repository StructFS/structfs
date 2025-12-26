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
//! - **`core`**: The main REPL loop, interacts only through `IoHost` trait
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

// Legacy implementations (to be deprecated)
pub mod commands;
pub mod completer;
pub mod core;
pub mod help_store;
pub mod highlighter;
pub mod host;
pub mod io;
pub mod register_store;
pub mod store_context;

// New architecture implementations
pub mod core_commands;
pub mod core_help_store;
pub mod core_repl;
pub mod core_store_context;

// Re-exports (legacy)
pub use core::ReplCore;
pub use host::TerminalHost;
pub use io::{ExitReason, IoHost, Output, PromptConfig, Signal};
pub use store_context::StoreContext;

// Re-exports (new architecture)
pub use core_repl::ReplCore as CoreReplCore;
pub use core_store_context::StoreContext as CoreStoreContext;

/// Run the REPL with the terminal host (new architecture).
///
/// This is the main entry point for the CLI application.
pub fn run() -> std::io::Result<()> {
    let mut core = CoreReplCore::new();
    let mut host = TerminalHost::new()?;

    match core.run(&mut host) {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Run the legacy REPL with the terminal host.
pub fn run_legacy() -> std::io::Result<()> {
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
