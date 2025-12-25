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
    ColumnarMenu, DefaultHinter, EditCommand, EditMode, Emacs, KeyCode, KeyModifiers, MenuBuilder,
    Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline,
    ReedlineEvent, ReedlineMenu, Signal as ReedlineSignal, Vi,
};

use crate::completer::ReplCompleter;
use crate::highlighter::ReplHighlighter;
use crate::io::{InputLine, IoError, IoHost, Output, OutputStyle, PromptConfig, Signal};

/// Terminal host using Reedline for interactive I/O.
pub struct TerminalHost {
    line_editor: Reedline,
    pending_input: Option<InputLine>,
    pending_signal: Option<Signal>,
    current_prompt: PromptConfig,
}

impl TerminalHost {
    /// Create a new terminal host.
    pub fn new() -> io::Result<Self> {
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
        let edit_mode: Box<dyn EditMode> = if should_use_vi_mode() {
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
            Err(e) => {
                return Err(IoError::Io(format!("Reedline error: {}", e)));
            }
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
        io::stdout().flush().map_err(|e| IoError::Io(e.to_string()))
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

/// Check if vi mode should be used based on environment configuration.
fn should_use_vi_mode() -> bool {
    // Check EDITOR environment variable
    if let Ok(editor) = std::env::var("EDITOR") {
        let editor_lower = editor.to_lowercase();
        if editor_lower.contains("vim") || editor_lower.contains("nvim") || editor_lower == "vi" {
            return true;
        }
    }

    // Check VISUAL environment variable
    if let Ok(visual) = std::env::var("VISUAL") {
        let visual_lower = visual.to_lowercase();
        if visual_lower.contains("vim") || visual_lower.contains("nvim") || visual_lower == "vi" {
            return true;
        }
    }

    // Check for set -o vi or set editing-mode vi in inputrc
    if check_inputrc_vi_mode() {
        return true;
    }

    // Check STRUCTFS_EDIT_MODE for explicit override
    if let Ok(mode) = std::env::var("STRUCTFS_EDIT_MODE") {
        return mode.to_lowercase() == "vi" || mode.to_lowercase() == "vim";
    }

    false
}

/// Check .inputrc for vi mode setting.
fn check_inputrc_vi_mode() -> bool {
    let inputrc_paths = [
        std::env::var("INPUTRC").ok().map(PathBuf::from),
        dirs::home_dir().map(|p| p.join(".inputrc")),
        Some(PathBuf::from("/etc/inputrc")),
    ];

    for path_opt in inputrc_paths.into_iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(&path_opt) {
            for line in content.lines() {
                let line = line.trim();
                // Check for "set editing-mode vi"
                if line.starts_with("set") && line.contains("editing-mode") && line.contains("vi") {
                    return true;
                }
            }
        }
    }

    false
}
