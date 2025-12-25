//! Host implementations for the REPL.
//!
//! This module contains platform-specific I/O implementations.
//! The terminal host uses Reedline for interactive terminal I/O.

pub mod terminal;

pub use terminal::TerminalHost;
