use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

use crate::command_table::{self, ArgKind};

/// Syntax highlighter for the REPL, driven by the command table.
pub struct ReplHighlighter;

impl ReplHighlighter {
    pub fn new() -> Self {
        Self
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
        let spec = command_table::lookup(command);
        let cmd_style = if spec.is_some() {
            Style::new().bold().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::Red)
        };
        styled.push((cmd_style, command.to_string()));

        if rest.is_empty() {
            return styled;
        }

        match spec.map_or(ArgKind::Text, |spec| spec.args) {
            ArgKind::PathAndValue => {
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
            ArgKind::Path => {
                styled.push((Style::new().fg(Color::Yellow), rest.to_string()));
            }
            ArgKind::Text => {
                styled.push((Style::new(), rest.to_string()));
            }
        }

        styled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_spelling_is_highlighted_as_a_command() {
        let highlighter = ReplHighlighter;
        for spec in crate::command_table::COMMANDS {
            for spelling in spec.spellings() {
                let styled = highlighter.highlight(spelling, 0);
                assert_eq!(
                    styled.buffer[0].0.foreground,
                    Some(Color::Cyan),
                    "{spelling}"
                );
            }
        }
    }

    #[test]
    fn ls_path_is_highlighted_as_a_path() {
        let styled = ReplHighlighter::new().highlight("ls /ctx", 0);
        assert_eq!(styled.buffer[1].0.foreground, Some(Color::Yellow));
    }

    #[test]
    fn highlight_empty_returns_empty() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("", 0);
        assert!(styled.buffer.is_empty());
    }

    #[test]
    fn highlight_recognized_command_only() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("help", 0);
        assert_eq!(styled.buffer.len(), 1);
        assert_eq!(styled.buffer[0].1, "help");
        // Cyan color for recognized commands
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_unknown_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("unknown", 0);
        assert_eq!(styled.buffer.len(), 1);
        assert_eq!(styled.buffer[0].1, "unknown");
        // Red color for unknown commands
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Red));
    }

    #[test]
    fn highlight_read_with_path() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("read /ctx/sys", 0);
        assert_eq!(styled.buffer.len(), 2);
        assert_eq!(styled.buffer[0].1, "read");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
        assert_eq!(styled.buffer[1].1, " /ctx/sys");
        assert_eq!(styled.buffer[1].0.foreground, Some(Color::Yellow));
    }

    #[test]
    fn highlight_write_with_path_only() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("write /path", 0);
        assert_eq!(styled.buffer.len(), 2);
        assert_eq!(styled.buffer[0].1, "write");
        assert_eq!(styled.buffer[1].1, " /path");
        assert_eq!(styled.buffer[1].0.foreground, Some(Color::Yellow));
    }

    #[test]
    fn highlight_write_with_json_object() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("write /path {\"key\": \"value\"}", 0);
        assert_eq!(styled.buffer.len(), 3);
        assert_eq!(styled.buffer[0].1, "write");
        assert_eq!(styled.buffer[1].1, " /path ");
        assert_eq!(styled.buffer[1].0.foreground, Some(Color::Yellow));
        assert_eq!(styled.buffer[2].1, "{\"key\": \"value\"}");
        assert_eq!(styled.buffer[2].0.foreground, Some(Color::Green));
    }

    #[test]
    fn highlight_write_with_json_array() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("write /path [1, 2, 3]", 0);
        assert_eq!(styled.buffer.len(), 3);
        assert_eq!(styled.buffer[2].1, "[1, 2, 3]");
        assert_eq!(styled.buffer[2].0.foreground, Some(Color::Green));
    }

    #[test]
    fn highlight_write_with_json_string() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("write /path \"hello\"", 0);
        assert_eq!(styled.buffer.len(), 3);
        assert_eq!(styled.buffer[2].1, "\"hello\"");
        assert_eq!(styled.buffer[2].0.foreground, Some(Color::Green));
    }

    #[test]
    fn highlight_set_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("set /path {}", 0);
        assert_eq!(styled.buffer[0].1, "set");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
        assert_eq!(styled.buffer[2].0.foreground, Some(Color::Green));
    }

    #[test]
    fn highlight_w_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("w /path {}", 0);
        assert_eq!(styled.buffer[0].1, "w");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_get_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("get /path", 0);
        assert_eq!(styled.buffer[0].1, "get");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
        assert_eq!(styled.buffer[1].0.foreground, Some(Color::Yellow));
    }

    #[test]
    fn highlight_r_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("r /path", 0);
        assert_eq!(styled.buffer[0].1, "r");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_cd_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("cd /ctx", 0);
        assert_eq!(styled.buffer[0].1, "cd");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
        assert_eq!(styled.buffer[1].0.foreground, Some(Color::Yellow));
    }

    #[test]
    fn highlight_help_with_args() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("help topic", 0);
        assert_eq!(styled.buffer.len(), 2);
        assert_eq!(styled.buffer[0].1, "help");
        assert_eq!(styled.buffer[1].1, " topic");
        // Rest is unstyled for non-read/write commands
        assert_eq!(styled.buffer[1].0.foreground, None);
    }

    #[test]
    fn highlight_exit_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("exit", 0);
        assert_eq!(styled.buffer[0].1, "exit");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_quit_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("quit", 0);
        assert_eq!(styled.buffer[0].1, "quit");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_q_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("q", 0);
        assert_eq!(styled.buffer[0].1, "q");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_pwd_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("pwd", 0);
        assert_eq!(styled.buffer[0].1, "pwd");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_mounts_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("mounts", 0);
        assert_eq!(styled.buffer[0].1, "mounts");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_ls_command() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("ls", 0);
        assert_eq!(styled.buffer[0].1, "ls");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_case_insensitive() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("READ /path", 0);
        assert_eq!(styled.buffer[0].1, "READ");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_mixed_case() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("ReAd /path", 0);
        assert_eq!(styled.buffer[0].1, "ReAd");
        assert_eq!(styled.buffer[0].0.foreground, Some(Color::Cyan));
    }

    #[test]
    fn highlight_command_is_bold() {
        let highlighter = ReplHighlighter::new();
        let styled = highlighter.highlight("read", 0);
        assert!(styled.buffer[0].0.is_bold);
    }
}
