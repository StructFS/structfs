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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_completer_with_commands() {
        let completer = ReplCompleter::new();
        assert!(completer.commands.contains(&"help".to_string()));
        assert!(completer.commands.contains(&"exit".to_string()));
        assert!(completer.commands.contains(&"quit".to_string()));
        assert!(completer.commands.contains(&"read".to_string()));
        assert!(completer.commands.contains(&"write".to_string()));
        assert!(completer.commands.contains(&"get".to_string()));
        assert!(completer.commands.contains(&"set".to_string()));
        assert!(completer.commands.contains(&"cd".to_string()));
        assert!(completer.commands.contains(&"pwd".to_string()));
        assert!(completer.commands.contains(&"mounts".to_string()));
    }

    #[test]
    fn default_creates_same_as_new() {
        let default: ReplCompleter = Default::default();
        let new = ReplCompleter::new();
        assert_eq!(default.commands, new.commands);
    }

    #[test]
    fn complete_empty_input_returns_all_commands() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("", 0);
        assert_eq!(suggestions.len(), completer.commands.len());
    }

    #[test]
    fn complete_partial_command_returns_matches() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("rea", 3);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "read");
    }

    #[test]
    fn complete_partial_command_multiple_matches() {
        let mut completer = ReplCompleter::new();
        // "e" matches "exit"
        let suggestions = completer.complete("e", 1);
        assert!(suggestions.iter().any(|s| s.value == "exit"));
    }

    #[test]
    fn complete_partial_command_q_matches_quit() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("q", 1);
        assert!(suggestions.iter().any(|s| s.value == "quit"));
    }

    #[test]
    fn complete_after_command_returns_empty() {
        let mut completer = ReplCompleter::new();
        // Space after command means we're on second word
        let suggestions = completer.complete("read ", 5);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn complete_second_word_returns_empty() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("read /path", 10);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn complete_unknown_prefix_returns_empty() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("xyz", 3);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn complete_exact_command_returns_it() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("help", 4);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "help");
    }

    #[test]
    fn complete_has_correct_span() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("rea", 3);
        assert_eq!(suggestions[0].span.start, 0);
        assert_eq!(suggestions[0].span.end, 3);
    }

    #[test]
    fn complete_has_append_whitespace() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("rea", 3);
        assert!(suggestions[0].append_whitespace);
    }

    #[test]
    fn complete_has_description() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("help", 4);
        assert_eq!(suggestions[0].description, Some("Show help".to_string()));
    }

    #[test]
    fn command_description_help() {
        assert_eq!(command_description("help"), "Show help");
    }

    #[test]
    fn command_description_exit() {
        assert_eq!(command_description("exit"), "Exit the REPL");
    }

    #[test]
    fn command_description_quit() {
        assert_eq!(command_description("quit"), "Exit the REPL");
    }

    #[test]
    fn command_description_read() {
        assert_eq!(command_description("read"), "Read from path");
    }

    #[test]
    fn command_description_get() {
        assert_eq!(command_description("get"), "Read from path");
    }

    #[test]
    fn command_description_write() {
        assert_eq!(command_description("write"), "Write to path");
    }

    #[test]
    fn command_description_set() {
        assert_eq!(command_description("set"), "Write to path");
    }

    #[test]
    fn command_description_cd() {
        assert_eq!(command_description("cd"), "Change directory");
    }

    #[test]
    fn command_description_pwd() {
        assert_eq!(command_description("pwd"), "Print working directory");
    }

    #[test]
    fn command_description_mounts() {
        assert_eq!(command_description("mounts"), "List current mounts");
    }

    #[test]
    fn command_description_unknown() {
        assert_eq!(command_description("unknown"), "");
    }

    #[test]
    fn complete_write_matches() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("w", 1);
        assert!(suggestions.iter().any(|s| s.value == "write"));
    }

    #[test]
    fn complete_set_matches() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("s", 1);
        assert!(suggestions.iter().any(|s| s.value == "set"));
    }

    #[test]
    fn complete_pwd_matches() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("p", 1);
        assert!(suggestions.iter().any(|s| s.value == "pwd"));
    }

    #[test]
    fn complete_mounts_matches() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("m", 1);
        assert!(suggestions.iter().any(|s| s.value == "mounts"));
    }
}
