use std::io::{self, Write};
use std::path::PathBuf;

use nu_ansi_term::{Color, Style};
use reedline::{
    default_emacs_keybindings, default_vi_insert_keybindings, default_vi_normal_keybindings,
    ColumnarMenu, DefaultHinter, EditCommand, EditMode, Emacs, KeyCode, KeyModifiers, MenuBuilder,
    Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline,
    ReedlineEvent, ReedlineMenu, Signal, Vi,
};

use crate::commands::{self, CommandResult};
use crate::completer::ReplCompleter;
use crate::highlighter::ReplHighlighter;
use crate::store_context::StoreContext;

/// The StructFS REPL prompt
struct ReplPrompt {
    mount_count: usize,
    path: String,
}

impl ReplPrompt {
    fn new(ctx: &StoreContext) -> Self {
        let mount_count = ctx.list_mounts().len();

        let path = if ctx.current_path().is_empty() {
            "/".to_string()
        } else {
            format!("/{}", ctx.current_path().components.join("/"))
        };

        Self { mount_count, path }
    }
}

impl Prompt for ReplPrompt {
    fn render_prompt_left(&self) -> std::borrow::Cow<'_, str> {
        let mount_info = if self.mount_count == 0 {
            Color::Yellow.paint("no mounts").to_string()
        } else {
            Color::Blue
                .bold()
                .paint(format!("{} mount(s)", self.mount_count))
                .to_string()
        };
        std::borrow::Cow::Owned(format!(
            "{} {}",
            mount_info,
            Color::Yellow.paint(&self.path)
        ))
    }

    fn render_prompt_right(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> std::borrow::Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => {
                std::borrow::Cow::Owned(format!("{} ", Color::Green.bold().paint(">")))
            }
            PromptEditMode::Vi(vi_mode) => {
                let indicator = match vi_mode {
                    reedline::PromptViMode::Normal => Color::Blue.bold().paint("[N]>"),
                    reedline::PromptViMode::Insert => Color::Green.bold().paint("[I]>"),
                };
                std::borrow::Cow::Owned(format!("{} ", indicator))
            }
            PromptEditMode::Custom(s) => std::borrow::Cow::Owned(format!("({})> ", s)),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(": ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> std::borrow::Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        std::borrow::Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }
}

/// Run the REPL
pub fn run() -> io::Result<()> {
    print_banner();

    let mut ctx = StoreContext::new();

    // Set up reedline with completion, hints, and highlighting
    let completer = Box::new(ReplCompleter::new());
    let highlighter = Box::new(ReplHighlighter::new());
    let hinter =
        Box::new(DefaultHinter::default().with_style(Style::new().fg(Color::LightGray).dimmed()));

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
        if let Ok(history) = reedline::FileBackedHistory::with_file(1000, history_path) {
            line_editor = line_editor.with_history(Box::new(history));
        }
    }

    loop {
        let prompt = ReplPrompt::new(&ctx);

        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(line)) => {
                let result = commands::execute(&line, &mut ctx);

                match result {
                    CommandResult::Ok(None) => {}
                    CommandResult::Ok(Some(output)) => {
                        println!("{}", output);
                    }
                    CommandResult::Error(msg) => {
                        println!("{} {}", Color::Red.bold().paint("Error:"), msg);
                    }
                    CommandResult::Help => {
                        println!("{}", commands::format_help());
                    }
                    CommandResult::Exit => {
                        println!("{}", Color::Cyan.paint("Goodbye!"));
                        break;
                    }
                }
            }
            Ok(Signal::CtrlC) => {
                println!("{}", Color::Yellow.paint("^C (use 'exit' to quit)"));
            }
            Ok(Signal::CtrlD) => {
                println!("{}", Color::Cyan.paint("Goodbye!"));
                break;
            }
            Err(err) => {
                println!("{} {}", Color::Red.paint("Error:"), err);
            }
        }

        // Flush stdout
        io::stdout().flush()?;
    }

    Ok(())
}

fn print_banner() {
    let banner = r#"
  _____ _                   _   _____ ____
 / ____| |                 | | |  ___/ ___|
| (___ | |_ _ __ _   _  ___| |_| |_  \___ \
 \___ \| __| '__| | | |/ __| __|  _|  ___) |
 ____) | |_| |  | |_| | (__| |_| |   |____/
|_____/ \__|_|   \__,_|\___|\___|_|
"#;
    println!("{}", Color::Cyan.paint(banner));
    println!(
        "{}",
        Style::new()
            .italic()
            .paint("Type 'help' for available commands, 'exit' to quit.\n")
    );
}

fn get_history_path() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|p| p.join("structfs").join("history.txt"))
}

/// Check if vi mode should be used based on environment configuration
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

/// Check .inputrc for vi mode setting
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
