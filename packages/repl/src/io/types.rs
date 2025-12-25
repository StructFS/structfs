//! I/O types for the REPL.
//!
//! These types define the interface between the REPL core and its host environment.

use serde::{Deserialize, Serialize};

/// A line of input from the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputLine {
    pub line: String,
}

/// A signal from the host (Ctrl+C, Ctrl+D, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "lowercase")]
pub enum Signal {
    /// User pressed Ctrl+C (interrupt).
    Interrupt,
    /// User pressed Ctrl+D (end of file).
    Eof,
}

/// Output to be written by the REPL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub text: String,
    #[serde(default)]
    pub style: OutputStyle,
}

impl Output {
    pub fn normal(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: OutputStyle::Normal,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: OutputStyle::Error,
        }
    }

    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: OutputStyle::Info,
        }
    }

    pub fn banner(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: OutputStyle::Banner,
        }
    }
}

/// Style hint for output rendering.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputStyle {
    /// Normal output (already contains ANSI codes if applicable).
    #[default]
    Normal,
    /// Error message (host may add red prefix).
    Error,
    /// Informational message (host may style in cyan).
    Info,
    /// Banner/startup message.
    Banner,
}

/// Prompt configuration sent from core to host.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptConfig {
    /// Number of active mounts.
    pub mount_count: usize,
    /// Current working path (formatted with leading /).
    pub current_path: String,
}

/// Reason the REPL exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitReason {
    /// User typed 'exit' or 'quit'.
    UserExit,
    /// User pressed Ctrl+D.
    Eof,
}
