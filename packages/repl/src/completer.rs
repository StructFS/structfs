use reedline::{Completer, Span, Suggestion};

/// Command completer for the REPL
pub struct ReplCompleter {
    commands: Vec<String>,
}

impl ReplCompleter {
    pub fn new() -> Self {
        Self {
            commands: vec![
                "help".to_string(),
                "exit".to_string(),
                "quit".to_string(),
                "read".to_string(),
                "write".to_string(),
                "get".to_string(),
                "set".to_string(),
                "cd".to_string(),
                "pwd".to_string(),
                "mounts".to_string(),
            ],
        }
    }
}

impl Default for ReplCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for ReplCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();

        // Get the word being typed
        let line_to_pos = &line[..pos];
        let words: Vec<&str> = line_to_pos.split_whitespace().collect();

        if words.is_empty() || (words.len() == 1 && !line_to_pos.ends_with(' ')) {
            // Completing the command itself
            let prefix = words.first().copied().unwrap_or("");
            let start = line_to_pos.rfind(prefix).unwrap_or(0);

            for cmd in &self.commands {
                if cmd.starts_with(prefix) {
                    suggestions.push(Suggestion {
                        value: cmd.clone(),
                        description: Some(command_description(cmd)),
                        style: None,
                        extra: None,
                        span: Span::new(start, pos),
                        append_whitespace: true,
                        match_indices: None,
                    });
                }
            }
        }

        suggestions
    }
}

fn command_description(cmd: &str) -> String {
    match cmd {
        "help" => "Show help".to_string(),
        "exit" | "quit" => "Exit the REPL".to_string(),
        "read" | "get" => "Read from path".to_string(),
        "write" | "set" => "Write to path".to_string(),
        "cd" => "Change directory".to_string(),
        "pwd" => "Print working directory".to_string(),
        "mounts" => "List current mounts".to_string(),
        _ => String::new(),
    }
}
