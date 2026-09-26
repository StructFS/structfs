//! Terminal host implementation using Reedline.
//!
//! This host provides interactive terminal I/O with:
//! - Readline-style line editing (Vi and Emacs modes)
//! - Tab completion
//! - Syntax highlighting
//! - Command history

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::PathBuf;

use nu_ansi_term::{Color, Style};
use reedline::{
    default_emacs_keybindings, default_vi_insert_keybindings, default_vi_normal_keybindings,
    ColumnarMenu, DefaultHinter, EditCommand, Emacs, KeyCode, KeyModifiers, MenuBuilder, Prompt,
    PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline, ReedlineEvent,
    ReedlineMenu, Signal as ReedlineSignal, Vi,
};

use crate::completer::ReplCompleter;
use crate::highlighter::ReplHighlighter;
use crate::io::{InputLine, IoError, IoHost, Output, OutputStyle, PromptConfig, Signal};

/// Line-editing key bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditMode {
    Vi,
    Emacs,
}

impl EditMode {
    /// Parse a mode name (`vi`, `vim`, `emacs`), case-insensitively.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "vi" | "vim" => Some(Self::Vi),
            "emacs" => Some(Self::Emacs),
            _ => None,
        }
    }
}

/// Terminal host using Reedline for interactive I/O.
pub struct TerminalHost {
    line_editor: Reedline,
    pending_input: Option<InputLine>,
    pending_signal: Option<Signal>,
    current_prompt: PromptConfig,
}

impl TerminalHost {
    /// Create a new terminal host, detecting the edit mode from the
    /// environment (see [`TerminalHost::with_edit_mode`]).
    pub fn new() -> io::Result<Self> {
        Self::with_edit_mode(None)
    }

    /// Create a new terminal host.
    ///
    /// `explicit` (from `--vi`/`--emacs`) wins; otherwise
    /// `STRUCTFS_EDIT_MODE`, then `EDITOR`/`VISUAL`, then readline's
    /// `set editing-mode` directive in the inputrc decide. The default is
    /// emacs.
    pub fn with_edit_mode(explicit: Option<EditMode>) -> io::Result<Self> {
        // Set up reedline with completion, hints, and highlighting
        let completer = Box::new(ReplCompleter::new());
        let highlighter = Box::new(ReplHighlighter::new());
        let hinter = Box::new(
            DefaultHinter::default().with_style(Style::new().fg(Color::LightGray).dimmed()),
        );

        // Create completion menu
        let completion_menu = Box::new(
            ColumnarMenu::default()
                .with_name("completion_menu")
                .with_text_style(Style::new().fg(Color::Cyan))
                .with_selected_text_style(Style::new().fg(Color::Black).on(Color::Cyan).bold()),
        );

        // Detect edit mode from environment
        let edit_mode: Box<dyn reedline::EditMode> = if detect_edit_mode(explicit) == EditMode::Vi {
            let mut insert_keybindings = default_vi_insert_keybindings();
            let normal_keybindings = default_vi_normal_keybindings();

            // Add tab completion in insert mode
            insert_keybindings.add_binding(
                KeyModifiers::NONE,
                KeyCode::Tab,
                ReedlineEvent::UntilFound(vec![
                    ReedlineEvent::Menu("completion_menu".to_string()),
                    ReedlineEvent::MenuNext,
                ]),
            );

            Box::new(Vi::new(insert_keybindings, normal_keybindings))
        } else {
            let mut keybindings = default_emacs_keybindings();
            keybindings.add_binding(
                KeyModifiers::NONE,
                KeyCode::Tab,
                ReedlineEvent::UntilFound(vec![
                    ReedlineEvent::Menu("completion_menu".to_string()),
                    ReedlineEvent::MenuNext,
                ]),
            );
            keybindings.add_binding(
                KeyModifiers::CONTROL,
                KeyCode::Char('d'),
                ReedlineEvent::Edit(vec![EditCommand::Clear]),
            );

            Box::new(Emacs::new(keybindings))
        };

        // Build the line editor
        let mut line_editor = Reedline::create()
            .with_completer(completer)
            .with_highlighter(highlighter)
            .with_hinter(hinter)
            .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
            .with_edit_mode(edit_mode);

        // Try to load history
        if let Some(history_path) = get_history_path() {
            // Ensure parent directory exists
            if let Some(parent) = history_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(history) = reedline::FileBackedHistory::with_file(1000, history_path) {
                line_editor = line_editor.with_history(Box::new(history));
            }
        }

        Ok(Self {
            line_editor,
            pending_input: None,
            pending_signal: None,
            current_prompt: PromptConfig::default(),
        })
    }
}

impl IoHost for TerminalHost {
    fn wait_for_input(&mut self) -> Result<(), IoError> {
        let prompt = TerminalPrompt::from_config(&self.current_prompt);

        match self.line_editor.read_line(&prompt) {
            Ok(ReedlineSignal::Success(line)) => {
                self.pending_input = Some(InputLine { line });
            }
            Ok(ReedlineSignal::CtrlC) => {
                self.pending_signal = Some(Signal::Interrupt);
            }
            Ok(ReedlineSignal::CtrlD) => {
                self.pending_signal = Some(Signal::Eof);
            }
            Err(e) => return Err(IoError::Io(e)),
        }

        Ok(())
    }

    fn read_input(&mut self) -> Result<Option<InputLine>, IoError> {
        Ok(self.pending_input.take())
    }

    fn read_signal(&mut self) -> Result<Option<Signal>, IoError> {
        Ok(self.pending_signal.take())
    }

    fn write_output(&mut self, output: Output) -> Result<(), IoError> {
        let styled = match output.style {
            OutputStyle::Normal => output.text,
            OutputStyle::Error => {
                format!("{} {}", Color::Red.bold().paint("Error:"), output.text)
            }
            OutputStyle::Info => Color::Cyan.paint(&output.text).to_string(),
            OutputStyle::Banner => Color::Cyan.paint(&output.text).to_string(),
        };
        println!("{}", styled);
        Ok(())
    }

    fn write_prompt(&mut self, config: PromptConfig) -> Result<(), IoError> {
        self.current_prompt = config;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(io::stdout().flush()?)
    }
}

/// Prompt implementation for the terminal.
struct TerminalPrompt {
    mount_count: usize,
    path: String,
}

impl TerminalPrompt {
    fn from_config(config: &PromptConfig) -> Self {
        Self {
            mount_count: config.mount_count,
            path: config.current_path.clone(),
        }
    }
}

impl Prompt for TerminalPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        let mount_info = if self.mount_count == 0 {
            Color::Yellow.paint("no mounts").to_string()
        } else {
            Color::Blue
                .bold()
                .paint(format!("{} mount(s)", self.mount_count))
                .to_string()
        };
        Cow::Owned(format!(
            "{} {}",
            mount_info,
            Color::Yellow.paint(&self.path)
        ))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => {
                Cow::Owned(format!("{} ", Color::Green.bold().paint(">")))
            }
            PromptEditMode::Vi(vi_mode) => {
                let indicator = match vi_mode {
                    reedline::PromptViMode::Normal => Color::Blue.bold().paint("[N]>"),
                    reedline::PromptViMode::Insert => Color::Green.bold().paint("[I]>"),
                };
                Cow::Owned(format!("{} ", indicator))
            }
            PromptEditMode::Custom(s) => Cow::Owned(format!("({})> ", s)),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(": ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }
}

fn get_history_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("structfs").join("history.txt"))
}

/// Pick the edit mode from the process environment.
fn detect_edit_mode(explicit: Option<EditMode>) -> EditMode {
    let env = |name| std::env::var(name).ok();
    choose_edit_mode(
        explicit,
        env("STRUCTFS_EDIT_MODE").as_deref(),
        env("EDITOR").as_deref(),
        env("VISUAL").as_deref(),
        read_inputrc().as_deref(),
    )
}

/// The edit-mode precedence, as a pure function: an explicit choice, then
/// `STRUCTFS_EDIT_MODE`, then a vi-family `EDITOR` or `VISUAL`, then the
/// inputrc's `set editing-mode` directive, then emacs.
fn choose_edit_mode(
    explicit: Option<EditMode>,
    structfs_edit_mode: Option<&str>,
    editor: Option<&str>,
    visual: Option<&str>,
    inputrc: Option<&str>,
) -> EditMode {
    if let Some(mode) = explicit.or_else(|| structfs_edit_mode.and_then(EditMode::parse)) {
        return mode;
    }
    let is_vi_editor = |program: &str| {
        let name = program
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_lowercase();
        matches!(name.as_str(), "vi" | "vim" | "nvim" | "gvim" | "mvim")
    };
    if editor.is_some_and(is_vi_editor) || visual.is_some_and(is_vi_editor) {
        return EditMode::Vi;
    }
    inputrc
        .and_then(inputrc_edit_mode)
        .unwrap_or(EditMode::Emacs)
}

/// The mode set by the last `set editing-mode <mode>` directive in an
/// inputrc, ignoring comments.
fn inputrc_edit_mode(content: &str) -> Option<EditMode> {
    content.lines().rev().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let mut words = line.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("set"), Some(var), Some(value)) if var.eq_ignore_ascii_case("editing-mode") => {
                EditMode::parse(value)
            }
            _ => None,
        }
    })
}

/// The inputrc readline would read: `$INPUTRC`, else `~/.inputrc`, else
/// `/etc/inputrc`.
fn read_inputrc() -> Option<String> {
    [
        std::env::var("INPUTRC").ok().map(PathBuf::from),
        dirs::home_dir().map(|p| p.join(".inputrc")),
        Some(PathBuf::from("/etc/inputrc")),
    ]
    .into_iter()
    .flatten()
    .find_map(|path| std::fs::read_to_string(path).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_choice_beats_the_environment() {
        let vi_rc = "set editing-mode vi\n";
        assert_eq!(
            choose_edit_mode(
                Some(EditMode::Emacs),
                Some("vi"),
                Some("vim"),
                Some("vim"),
                Some(vi_rc)
            ),
            EditMode::Emacs
        );
        assert_eq!(
            choose_edit_mode(None, Some("emacs"), Some("vim"), None, Some(vi_rc)),
            EditMode::Emacs
        );
        assert_eq!(
            choose_edit_mode(Some(EditMode::Vi), None, None, None, None),
            EditMode::Vi
        );
    }

    #[test]
    fn editor_then_inputrc_then_emacs() {
        assert_eq!(
            choose_edit_mode(None, None, Some("/usr/bin/nvim -u NONE"), None, None),
            EditMode::Vi
        );
        assert_eq!(
            choose_edit_mode(None, Some("bogus"), None, Some("vi"), None),
            EditMode::Vi
        );
        assert_eq!(
            choose_edit_mode(None, None, Some("nano"), None, Some("set editing-mode vi")),
            EditMode::Vi
        );
        assert_eq!(
            choose_edit_mode(None, None, Some("code --wait"), None, None),
            EditMode::Emacs
        );
    }

    #[test]
    fn inputrc_directive_is_parsed_not_substring_matched() {
        assert_eq!(inputrc_edit_mode("set editing-mode vi"), Some(EditMode::Vi));
        assert_eq!(
            inputrc_edit_mode("  set  editing-mode   emacs  "),
            Some(EditMode::Emacs)
        );
        // Comments and unrelated settings that mention both words do not count.
        assert_eq!(inputrc_edit_mode("# set editing-mode vi"), None);
        assert_eq!(
            inputrc_edit_mode("set show-mode-in-prompt on # editing-mode vi"),
            None
        );
        assert_eq!(inputrc_edit_mode("set keymap vi-command"), None);
        // The last directive wins.
        assert_eq!(
            inputrc_edit_mode("set editing-mode vi\nset editing-mode emacs\n"),
            Some(EditMode::Emacs)
        );
    }
}
