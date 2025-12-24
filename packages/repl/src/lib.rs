//! # structfs-repl
//!
//! An interactive REPL for StructFS operations.
//!
//! This crate provides a command-line interface for reading and writing
//! to StructFS stores using JSON serialization.
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
//! > open ~/mydata
//! > read /users/1
//! > write /users/2 {"name": "Alice", "email": "alice@example.com"}
//! > connect https://api.example.com
//! > read /status
//! ```

pub mod commands;
pub mod completer;
pub mod help_store;
pub mod highlighter;
pub mod repl;
pub mod store_context;

pub use repl::run;
pub use store_context::StoreContext;
