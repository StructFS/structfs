use reedline::{Completer, Span, Suggestion};

use crate::command_table::{CommandSpec, COMMANDS};

/// Command completer for the REPL, driven by the command table.
///
/// Offers each command's name and its multi-letter aliases (single-letter
/// aliases such as `r` complete to nothing useful).
pub struct ReplCompleter {
    commands: Vec<(&'static str, &'static CommandSpec)>,
}

impl ReplCompleter {
    pub fn new() -> Self {
        Self {
            commands: COMMANDS
                .iter()
                .flat_map(|spec| {
                    spec.spellings()
                        .filter(|s| s.len() > 1)
                        .map(move |s| (s, spec))
                })
                .collect(),
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

            for (spelling, spec) in &self.commands {
                if spelling.starts_with(prefix) {
                    suggestions.push(Suggestion {
                        value: spelling.to_string(),
                        description: Some(spec.summary.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn values(suggestions: &[Suggestion]) -> Vec<&str> {
        suggestions.iter().map(|s| s.value.as_str()).collect()
    }

    #[test]
    fn offers_every_command_and_multi_letter_alias() {
        let mut completer = ReplCompleter::new();
        let all = completer.complete("", 0);
        let all = values(&all);
        for spec in COMMANDS {
            assert!(all.contains(&spec.name), "{} missing", spec.name);
        }
        for alias in ["get", "set", "quit", "regs"] {
            assert!(all.contains(&alias), "{alias} missing");
        }
        assert!(!all.contains(&"r") && !all.contains(&"q"));
        assert!(all.contains(&"ls") && all.contains(&"mounts"));
    }

    #[test]
    fn default_creates_same_as_new() {
        let default: ReplCompleter = Default::default();
        assert_eq!(default.commands.len(), ReplCompleter::new().commands.len());
    }

    #[test]
    fn complete_partial_command_returns_matches() {
        let mut completer = ReplCompleter::new();
        let suggestions = completer.complete("rea", 3);
        assert_eq!(values(&suggestions), vec!["read"]);
        assert_eq!(suggestions[0].span.start, 0);
        assert_eq!(suggestions[0].span.end, 3);
        assert!(suggestions[0].append_whitespace);
        assert_eq!(
            suggestions[0].description.as_deref(),
            Some(COMMANDS[0].summary)
        );
    }

    #[test]
    fn complete_prefixes() {
        let mut completer = ReplCompleter::new();
        for (prefix, expected) in [
            ("e", "exit"),
            ("q", "quit"),
            ("w", "write"),
            ("s", "set"),
            ("p", "pwd"),
            ("m", "mounts"),
            ("l", "ls"),
        ] {
            let suggestions = completer.complete(prefix, prefix.len());
            assert!(
                values(&suggestions).contains(&expected),
                "{prefix} -> {expected}"
            );
        }
    }

    #[test]
    fn only_the_first_word_completes() {
        let mut completer = ReplCompleter::new();
        assert!(completer.complete("read ", 5).is_empty());
        assert!(completer.complete("read /path", 10).is_empty());
        assert!(completer.complete("xyz", 3).is_empty());
        assert_eq!(values(&completer.complete("help", 4)), vec!["help"]);
    }
}
