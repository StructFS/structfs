//! I/O abstraction for the REPL.
//!
//! This module defines the interface between the REPL core and its host environment.
//! The core interacts only through the `IoHost` trait, allowing different hosts
//! (terminal, Wasm, testing) to provide their own implementations.

pub mod types;

#[cfg(test)]
pub mod test_host;

pub use types::*;

#[cfg(test)]
pub use test_host::TestHost;

/// Error type for I/O operations.
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("I/O error: {0}")]
    Io(String),
}

/// Host interface for REPL I/O operations.
///
/// The REPL core calls these methods to interact with the user.
/// Different hosts can implement this trait to provide terminal I/O,
/// browser-based I/O, or mock I/O for testing.
pub trait IoHost {
    /// Wait for input to become available.
    ///
    /// This may block (for terminal hosts) or return immediately (for async hosts).
    /// After this returns, `read_input()` should return `Some(InputLine)` if
    /// input is ready, or `read_signal()` should return `Some(Signal)` if a
    /// signal was received.
    fn wait_for_input(&mut self) -> Result<(), IoError>;

    /// Read the next input line, if available.
    ///
    /// Returns `None` if no input is ready. Call `wait_for_input()` first
    /// to ensure input is available.
    fn read_input(&mut self) -> Result<Option<InputLine>, IoError>;

    /// Read any pending signal (Ctrl+C, Ctrl+D).
    ///
    /// Returns `None` if no signal is pending.
    fn read_signal(&mut self) -> Result<Option<Signal>, IoError>;

    /// Write output to the user.
    fn write_output(&mut self, output: Output) -> Result<(), IoError>;

    /// Update the prompt configuration.
    ///
    /// The host uses this to render the prompt before the next input.
    fn write_prompt(&mut self, config: PromptConfig) -> Result<(), IoError>;

    /// Flush any buffered output.
    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}
