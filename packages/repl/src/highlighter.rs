use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

/// Syntax highlighter for the REPL
pub struct ReplHighlighter {
    commands: Vec<&'static str>,
}

impl ReplHighlighter {
    pub fn new() -> Self {
        Self {
            commands: vec![
                "help", "exit", "quit", "q", "read", "write", "get", "set", "r", "w", "cd", "pwd",
                "mounts", "ls",
            ],
        }
    }
}

impl Default for ReplHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter for ReplHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();

        if line.is_empty() {
            return styled;
        }

        // Find the first whitespace to split command from args
        let (command, rest) = match line.find(char::is_whitespace) {
            Some(pos) => (&line[..pos], &line[pos..]),
            None => (line, ""),
        };

        // Highlight the command
        let cmd_lower = command.to_lowercase();
        let cmd_style = if self.commands.contains(&cmd_lower.as_str()) {
            Style::new().bold().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::Red)
        };
        styled.push((cmd_style, command.to_string()));

        if rest.is_empty() {
            return styled;
        }

        // For write commands, try to highlight path vs JSON simply
        match cmd_lower.as_str() {
            "write" | "set" | "w" => {
                // Find first JSON-starting character
                if let Some(json_pos) = rest.find(['{', '[', '"']) {
                    // Everything before JSON start is path (with leading whitespace)
                    let before_json = &rest[..json_pos];
                    let json_part = &rest[json_pos..];

                    styled.push((Style::new().fg(Color::Yellow), before_json.to_string()));
                    styled.push((Style::new().fg(Color::Green), json_part.to_string()));
                } else {
                    // No JSON found, color as path
                    styled.push((Style::new().fg(Color::Yellow), rest.to_string()));
                }
            }
            "read" | "get" | "r" | "cd" => {
                styled.push((Style::new().fg(Color::Yellow), rest.to_string()));
            }
            _ => {
                styled.push((Style::new(), rest.to_string()));
            }
        }

        styled
    }
}
