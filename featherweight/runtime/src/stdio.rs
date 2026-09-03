//! Standard streams for blocks (`isotope/spec/09-posix-closure.md`).
//!
//! `iso/stdio/{stdin,stdout,stderr}` are served through one of these
//! backends. Which block gets which backend is a runtime decision: the
//! `fw` CLI attaches the host terminal to blocks declaring
//! `stdio: host`; tests inject [`ScriptedStdio`]; everything else gets
//! [`NullStdio`] (EOF on stdin, output to the log).

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// A block's standard streams. Line/value-oriented at this layer.
pub trait Stdio: Send + Sync {
    /// Blocking: the next input line (without the newline); `None` at EOF.
    fn read_line(&self) -> Option<String>;

    /// Append to standard output.
    fn write_out(&self, text: &str);

    /// Append to standard error.
    fn write_err(&self, text: &str);
}

/// No terminal: stdin is at EOF; output is discarded (the runtime's log
/// sink already carries `iso/log`).
pub struct NullStdio;

impl Stdio for NullStdio {
    fn read_line(&self) -> Option<String> {
        None
    }

    fn write_out(&self, _text: &str) {}

    fn write_err(&self, _text: &str) {}
}

/// The host process's terminal.
pub struct HostStdio;

impl Stdio for HostStdio {
    fn read_line(&self) -> Option<String> {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                Some(line)
            }
        }
    }

    fn write_out(&self, text: &str) {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(text.as_bytes());
        let _ = stdout.flush();
    }

    fn write_err(&self, text: &str) {
        let mut stderr = std::io::stderr();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
    }
}

/// Scripted streams for tests: canned input lines, captured output.
#[derive(Clone, Default)]
pub struct ScriptedStdio {
    input: Arc<Mutex<VecDeque<String>>>,
    output: Arc<Mutex<String>>,
}

impl ScriptedStdio {
    /// Create with input lines to feed the block.
    pub fn with_input(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            input: Arc::new(Mutex::new(lines.into_iter().map(Into::into).collect())),
            output: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Everything written to stdout and stderr so far.
    pub fn output(&self) -> String {
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Stdio for ScriptedStdio {
    fn read_line(&self) -> Option<String> {
        self.input
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }

    fn write_out(&self, text: &str) {
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_str(text);
    }

    fn write_err(&self, text: &str) {
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_str(text);
    }
}
