//! REPL command parsing and execution.
//!
//! Commands:
//! - `read [path]` - Read and display JSON at path
//! - `write <path> <json>` - Write JSON to path
//! - `cd <path>` - Change current path
//! - `pwd` - Print current path
//! - `help` - Show help
//! - `exit` - Exit the REPL
//!
//! Mount operations are done via writes to `/_mounts/*`:
//! - `write /_mounts/data {"type": "memory"}` - Mount memory store at /data
//! - `write /_mounts/files {"type": "local", "path": "/path"}` - Mount local store
//! - `write /_mounts/api {"type": "http", "url": "https://..."}` - Mount HTTP client
//! - `write /_mounts/remote {"type": "structfs", "url": "https://..."}` - Mount remote StructFS
//! - `read /_mounts` - List all mounts
//! - `write /_mounts/data null` - Unmount

use nu_ansi_term::{Color, Style};
use serde_json::Value as JsonValue;

use structfs_store::Path;

use crate::store_context::StoreContext;

/// Result of executing a command
pub enum CommandResult {
    /// Command succeeded, optionally with output to display
    Ok(Option<String>),
    /// Command failed with an error message
    Error(String),
    /// User requested to exit
    Exit,
    /// Show help
    Help,
}

/// Parse and execute a command
pub fn execute(input: &str, ctx: &mut StoreContext) -> CommandResult {
    let input = input.trim();

    if input.is_empty() {
        return CommandResult::Ok(None);
    }

    // Parse command and arguments
    let mut parts = input.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("").trim();

    match command.to_lowercase().as_str() {
        "help" | "?" => CommandResult::Help,
        "exit" | "quit" | "q" => CommandResult::Exit,
        "read" | "get" | "r" => cmd_read(args, ctx),
        "write" | "set" | "w" => cmd_write(args, ctx),
        "cd" => cmd_cd(args, ctx),
        "pwd" => cmd_pwd(ctx),
        "mounts" | "ls" => cmd_mounts(ctx),
        _ => CommandResult::Error(format!(
            "Unknown command: '{}'. Type 'help' for available commands.",
            command
        )),
    }
}

/// Format help text
pub fn format_help() -> String {
    let cmd_style = Style::new().bold().fg(Color::Cyan);
    let arg_style = Style::new().fg(Color::Yellow);
    let desc_style = Style::new().fg(Color::White);

    let mut help = String::new();
    help.push_str(&format!(
        "{}\n\n",
        Style::new().bold().paint("StructFS REPL Commands")
    ));

    let commands = [
        (
            "read",
            "[path]",
            "Read and display JSON at path (alias: get, r)",
        ),
        (
            "write",
            "<path> <json>",
            "Write JSON to path (alias: set, w)",
        ),
        ("cd", "<path>", "Change current path"),
        ("pwd", "", "Print current path"),
        ("mounts", "", "List current mounts (alias: ls)"),
        ("", "", ""),
        ("help", "", "Show this help message"),
        ("exit", "", "Exit the REPL (alias: quit, q)"),
    ];

    for (cmd, args, desc) in commands {
        if cmd.is_empty() {
            help.push('\n');
        } else {
            help.push_str(&format!(
                "  {:<12} {:<20} {}\n",
                cmd_style.paint(cmd),
                arg_style.paint(args),
                desc_style.paint(desc)
            ));
        }
    }

    help.push_str(&format!(
        "\n{}\n",
        Style::new().bold().paint("Mounting Stores")
    ));
    help.push_str(&format!(
        "  Mount a memory store:     {}\n",
        arg_style.paint("write /_mounts/data {\"type\": \"memory\"}")
    ));
    help.push_str(&format!(
        "  Mount a local directory:  {}\n",
        arg_style.paint("write /_mounts/files {\"type\": \"local\", \"path\": \"/path/to/dir\"}")
    ));
    help.push_str(&format!(
        "  Mount an HTTP client:     {}\n",
        arg_style
            .paint("write /_mounts/api {\"type\": \"http\", \"url\": \"https://api.example.com\"}")
    ));
    help.push_str(&format!(
        "  Mount a remote StructFS:  {}\n",
        arg_style.paint("write /_mounts/remote {\"type\": \"structfs\", \"url\": \"https://structfs.example.com\"}")
    ));
    help.push_str(&format!(
        "  List mounts:              {}\n",
        arg_style.paint("read /_mounts")
    ));
    help.push_str(&format!(
        "  Unmount:                  {}\n",
        arg_style.paint("write /_mounts/data null")
    ));

    help.push_str(&format!(
        "\n{}",
        Style::new()
            .italic()
            .paint("Paths: Use '/' for root, '..' to go up, or relative paths")
    ));

    help
}

fn cmd_read(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "." } else { args };

    let path = match ctx.resolve_path(path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
    };

    match ctx.read(&path) {
        Ok(Some(value)) => CommandResult::Ok(Some(format_json(&value))),
        Ok(None) => CommandResult::Ok(Some(format!(
            "{}",
            Color::Yellow.paint("null (path does not exist or no store mounted)")
        ))),
        Err(e) => CommandResult::Error(format!("Read error: {}", e)),
    }
}

fn cmd_write(args: &str, ctx: &mut StoreContext) -> CommandResult {
    // Parse path and JSON from args
    let (path_str, json_str) = match parse_write_args(args) {
        Some(parts) => parts,
        None => {
            return CommandResult::Error(
                "Usage: write <path> <json>\nExample: write /users/1 {\"name\": \"Alice\"}"
                    .to_string(),
            )
        }
    };

    let path = match ctx.resolve_path(&path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
    };

    let value: JsonValue = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => return CommandResult::Error(format!("Invalid JSON: {}", e)),
    };

    match ctx.write(&path, &value) {
        Ok(result_path) => {
            // If the store returned a relative path, make it absolute by prepending
            // the destination path (stores don't know where they're mounted)
            let full_path = if result_path.has_prefix(&path) {
                result_path.clone()
            } else {
                path.join(&result_path)
            };

            // Check if the result path differs from the write destination
            // (indicates a broker-style store that returns a handle)
            let output = if full_path != path && !result_path.is_empty() {
                format!(
                    "{}\n{} {}\n{} {}",
                    Color::Green.paint("ok"),
                    Color::Cyan.paint("result path:"),
                    format_path(&full_path),
                    Color::DarkGray.paint("(read from this path to get the result)"),
                    ""
                )
            } else {
                format!("{}", Color::Green.paint("ok"))
            };
            CommandResult::Ok(Some(output))
        }
        Err(e) => CommandResult::Error(format!("Write error: {}", e)),
    }
}

fn cmd_cd(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "/" } else { args };

    let path = match ctx.resolve_path(path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
    };

    ctx.set_current_path(path);
    CommandResult::Ok(None)
}

fn cmd_pwd(ctx: &mut StoreContext) -> CommandResult {
    CommandResult::Ok(Some(format_path(ctx.current_path())))
}

fn cmd_mounts(ctx: &mut StoreContext) -> CommandResult {
    let mounts = ctx.list_mounts();
    if mounts.is_empty() {
        CommandResult::Ok(Some(format!(
            "{}",
            Color::Yellow.paint(
                "No mounts. Use 'write /_mounts/<name> {\"type\": \"memory\"}' to mount a store."
            )
        )))
    } else {
        let mut output = String::new();
        for mount in mounts {
            output.push_str(&format!(
                "  {} -> {:?}\n",
                Color::Cyan.paint(&mount.path),
                mount.config
            ));
        }
        CommandResult::Ok(Some(output.trim_end().to_string()))
    }
}

/// Parse write command arguments into (path, json)
fn parse_write_args(args: &str) -> Option<(String, String)> {
    let args = args.trim();
    if args.is_empty() {
        return None;
    }

    // First, find whitespace that separates path from JSON
    // The path cannot contain '{', '[', or '"', so look for those as definite JSON starts
    // For 'true', 'false', 'null', and numbers, we need to find them after whitespace

    // Strategy: Look for the first occurrence of:
    // - '{' or '[' or '"' (definite JSON)
    // - or whitespace followed by a digit, 't', 'f', 'n' (possible JSON literal)

    let mut json_start = None;

    // First check for definite JSON starters
    for (i, c) in args.char_indices() {
        if c == '{' || c == '[' || c == '"' {
            json_start = Some(i);
            break;
        }
    }

    // If not found, look for whitespace followed by a JSON literal
    if json_start.is_none() {
        let chars: Vec<char> = args.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            if chars[i].is_whitespace() {
                let next = chars[i + 1];
                if next.is_ascii_digit() || next == '-' {
                    json_start = Some(i + 1);
                    break;
                }
                // Check for true/false/null
                let rest = &args[i + 1..];
                if rest.starts_with("true") || rest.starts_with("false") || rest.starts_with("null")
                {
                    json_start = Some(i + 1);
                    break;
                }
            }
        }
    }

    let json_start = json_start?;

    // Get the path (everything before JSON start, trimmed)
    let path = args[..json_start].trim().to_string();
    let json = args[json_start..].to_string();

    if path.is_empty() || json.is_empty() {
        return None;
    }

    Some((path, json))
}

/// Format a path for display
fn format_path(path: &Path) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", path.components.join("/"))
    }
}

/// Format JSON with syntax highlighting
fn format_json(value: &JsonValue) -> String {
    // Pretty print with indentation
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());

    // Apply basic syntax highlighting
    let mut result = String::new();
    let mut in_string = false;
    let mut escape_next = false;

    for c in pretty.chars() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        if c == '\\' && in_string {
            result.push(c);
            escape_next = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            result.push_str(&format!("{}", Color::Green.paint("\"")));
            continue;
        }

        if in_string {
            result.push_str(&format!("{}", Color::Green.paint(c.to_string())));
        } else {
            match c {
                '{' | '}' | '[' | ']' => {
                    result.push_str(&format!("{}", Color::White.bold().paint(c.to_string())))
                }
                ':' => result.push_str(&format!("{}", Color::White.paint(":"))),
                ',' => result.push_str(&format!("{}", Color::White.paint(","))),
                _ if c.is_ascii_digit() || c == '.' || c == '-' => {
                    result.push_str(&format!("{}", Color::Cyan.paint(c.to_string())))
                }
                _ => result.push(c),
            }
        }
    }

    // Handle null, true, false keywords
    result = result
        .replace("null", &format!("{}", Color::Yellow.paint("null")))
        .replace("true", &format!("{}", Color::Yellow.paint("true")))
        .replace("false", &format!("{}", Color::Yellow.paint("false")));

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_write_args() {
        assert_eq!(
            parse_write_args("/foo {\"bar\": 1}"),
            Some(("/foo".to_string(), "{\"bar\": 1}".to_string()))
        );

        assert_eq!(
            parse_write_args("path/to/thing 123"),
            Some(("path/to/thing".to_string(), "123".to_string()))
        );

        assert_eq!(
            parse_write_args("/x true"),
            Some(("/x".to_string(), "true".to_string()))
        );

        assert_eq!(parse_write_args(""), None);
        assert_eq!(parse_write_args("{\"only\": \"json\"}"), None);
    }
}
