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
//! Mount operations are done via writes to `/ctx/mounts/*`:
//! - `write /ctx/mounts/data {"type": "memory"}` - Mount memory store at /data
//! - `write /ctx/mounts/files {"type": "local", "path": "/path"}` - Mount local store
//! - `write /ctx/mounts/api {"type": "http", "url": "https://..."}` - Mount HTTP client
//! - `write /ctx/mounts/remote {"type": "structfs", "url": "https://..."}` - Mount remote StructFS
//! - `read /ctx/mounts` - List all mounts
//! - `write /ctx/mounts/data null` - Unmount

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

    // Check for register output capture: @name command ...
    if let Some((register_name, rest)) = parse_register_capture(input) {
        return execute_with_capture(&register_name, rest, ctx);
    }

    execute_command(input, ctx)
}

/// Parse a register capture prefix: @name followed by a command
/// Returns (register_name, remaining_input) if found
fn parse_register_capture(input: &str) -> Option<(String, &str)> {
    if !input.starts_with('@') {
        return None;
    }

    // Find whitespace after the register name
    let rest = &input[1..];
    let space_pos = rest.find(char::is_whitespace)?;

    let register_name = &rest[..space_pos];
    if register_name.is_empty() || register_name.contains('/') {
        // Empty name or contains path separator - not a capture, might be a path
        return None;
    }

    let remaining = rest[space_pos..].trim_start();
    if remaining.is_empty() {
        return None;
    }

    // Check if remaining starts with a valid command
    let first_word = remaining.split_whitespace().next()?;
    let is_command = matches!(
        first_word.to_lowercase().as_str(),
        "read" | "get" | "r" | "write" | "set" | "w" | "cd" | "pwd" | "mounts" | "ls"
    );

    if is_command {
        Some((register_name.to_string(), remaining))
    } else {
        None
    }
}

/// Execute a command and capture its JSON output to a register
fn execute_with_capture(register_name: &str, input: &str, ctx: &mut StoreContext) -> CommandResult {
    // Execute the inner command
    let result = execute_command(input, ctx);

    match result {
        CommandResult::Ok(Some(ref output)) => {
            // Try to parse the output as JSON (strip ANSI codes first)
            let plain_output = strip_ansi_codes(output);

            // Try to parse as JSON
            match serde_json::from_str::<JsonValue>(&plain_output) {
                Ok(value) => {
                    ctx.set_register(register_name, value);
                    CommandResult::Ok(Some(format!(
                        "{}\n{} {}",
                        output,
                        Color::Magenta.paint("→"),
                        Color::Magenta.paint(format!("@{}", register_name))
                    )))
                }
                Err(_) => {
                    // Store as string if not valid JSON
                    ctx.set_register(register_name, JsonValue::String(plain_output));
                    CommandResult::Ok(Some(format!(
                        "{}\n{} {} {}",
                        output,
                        Color::Magenta.paint("→"),
                        Color::Magenta.paint(format!("@{}", register_name)),
                        Color::DarkGray.paint("(stored as string)")
                    )))
                }
            }
        }
        CommandResult::Ok(None) => {
            // No output to capture
            ctx.set_register(register_name, JsonValue::Null);
            CommandResult::Ok(Some(format!(
                "{} {} {}",
                Color::Magenta.paint("→"),
                Color::Magenta.paint(format!("@{}", register_name)),
                Color::DarkGray.paint("(null)")
            )))
        }
        other => other,
    }
}

/// Strip ANSI escape codes from a string
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we find 'm' (end of ANSI sequence)
            while let Some(&next) = chars.peek() {
                chars.next();
                if next == 'm' {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Execute a command without register capture
fn execute_command(input: &str, ctx: &mut StoreContext) -> CommandResult {
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
        "registers" | "regs" => cmd_registers(ctx),
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
            "[path|@reg]",
            "Read JSON from path or register (alias: get, r)",
        ),
        (
            "write",
            "<path> <json|@reg>",
            "Write JSON or register to path (alias: set, w)",
        ),
        ("cd", "<path>", "Change current path"),
        ("pwd", "", "Print current path"),
        ("mounts", "", "List current mounts (alias: ls)"),
        ("registers", "", "List all registers (alias: regs)"),
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
        arg_style.paint("write /ctx/mounts/data {\"type\": \"memory\"}")
    ));
    help.push_str(&format!(
        "  Mount a local directory:  {}\n",
        arg_style
            .paint("write /ctx/mounts/files {\"type\": \"local\", \"path\": \"/path/to/dir\"}")
    ));
    help.push_str(&format!(
        "  Mount an HTTP client:     {}\n",
        arg_style.paint(
            "write /ctx/mounts/api {\"type\": \"http\", \"url\": \"https://api.example.com\"}"
        )
    ));
    help.push_str(&format!(
        "  Mount a remote StructFS:  {}\n",
        arg_style.paint("write /ctx/mounts/remote {\"type\": \"structfs\", \"url\": \"https://structfs.example.com\"}")
    ));
    help.push_str(&format!(
        "  List mounts:              {}\n",
        arg_style.paint("read /ctx/mounts")
    ));
    help.push_str(&format!(
        "  Unmount:                  {}\n",
        arg_style.paint("write /ctx/mounts/data null")
    ));

    help.push_str(&format!("\n{}\n", Style::new().bold().paint("Registers")));
    help.push_str(&format!(
        "  Store output:             {}\n",
        arg_style.paint("@result read /some/path")
    ));
    help.push_str(&format!(
        "  Read register:            {}\n",
        arg_style.paint("read @result")
    ));
    help.push_str(&format!(
        "  Read sub-path:            {}\n",
        arg_style.paint("read @result/nested/field")
    ));
    help.push_str(&format!(
        "  Write from register:      {}\n",
        arg_style.paint("write /dest @source")
    ));
    help.push_str(&format!(
        "  Write to register:        {}\n",
        arg_style.paint("write @temp {\"key\": \"value\"}")
    ));

    help.push_str(&format!(
        "\n{}",
        Style::new()
            .italic()
            .paint("Paths: Use '/' for root, '..' to go up, '@name' for registers")
    ));

    help
}

fn cmd_read(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "." } else { args };

    // Check if this is a register path
    if StoreContext::is_register_path(path_str) {
        match ctx.read_register(path_str) {
            Ok(Some(value)) => return CommandResult::Ok(Some(format_json(&value))),
            Ok(None) => {
                return CommandResult::Ok(Some(format!(
                    "{}",
                    Color::Yellow.paint("null (register does not exist)")
                )))
            }
            Err(e) => return CommandResult::Error(format!("Read error: {}", e)),
        }
    }

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
    // Parse path and value from args
    let (path_str, value_str) = match parse_write_args(args) {
        Some(parts) => parts,
        None => {
            return CommandResult::Error(
                "Usage: write <path> <json|@register>\nExample: write /users/1 {\"name\": \"Alice\"}\n         write /dest @source"
                    .to_string(),
            )
        }
    };

    // Get the value - either from JSON or from a register
    let value: JsonValue = if let Some(reg_name) = value_str.strip_prefix('@') {
        // Read from register
        match ctx.read_register(&value_str) {
            Ok(Some(v)) => v,
            Ok(None) => {
                return CommandResult::Error(format!("Register '{}' does not exist", reg_name))
            }
            Err(e) => return CommandResult::Error(format!("Error reading register: {}", e)),
        }
    } else {
        // Parse as JSON
        match serde_json::from_str(&value_str) {
            Ok(v) => v,
            Err(e) => return CommandResult::Error(format!("Invalid JSON: {}", e)),
        }
    };

    // Check if destination is a register
    if StoreContext::is_register_path(&path_str) {
        match ctx.write_register(&path_str, &value) {
            Ok(_) => {
                return CommandResult::Ok(Some(format!(
                    "{} {}",
                    Color::Green.paint("ok"),
                    Color::Magenta.paint(&path_str)
                )))
            }
            Err(e) => return CommandResult::Error(format!("Write error: {}", e)),
        }
    }

    let path = match ctx.resolve_path(&path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
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
                "No mounts. Use 'write /ctx/mounts/<name> {\"type\": \"memory\"}' to mount a store."
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

fn cmd_registers(ctx: &mut StoreContext) -> CommandResult {
    let registers = ctx.list_registers();
    if registers.is_empty() {
        CommandResult::Ok(Some(format!(
            "{}",
            Color::Yellow
                .paint("No registers. Use '@name read <path>' to store output in a register.")
        )))
    } else {
        let mut output = String::new();
        for name in registers {
            output.push_str(&format!(
                "  {}\n",
                Color::Magenta.paint(format!("@{}", name))
            ));
        }
        CommandResult::Ok(Some(output.trim_end().to_string()))
    }
}

/// Parse write command arguments into (path, value)
/// Value can be JSON or a register reference (@name or @name/path)
fn parse_write_args(args: &str) -> Option<(String, String)> {
    let args = args.trim();
    if args.is_empty() {
        return None;
    }

    // Strategy: Look for the value start, which can be:
    // - '{' or '[' or '"' (definite JSON)
    // - whitespace followed by a digit, 't', 'f', 'n' (possible JSON literal)
    // - whitespace followed by '@' (register reference)

    let mut value_start = None;

    // First check for definite JSON starters
    for (i, c) in args.char_indices() {
        if c == '{' || c == '[' || c == '"' {
            value_start = Some(i);
            break;
        }
    }

    // If not found, look for whitespace followed by a JSON literal or register reference
    if value_start.is_none() {
        let chars: Vec<char> = args.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            if chars[i].is_whitespace() {
                let next = chars[i + 1];
                // Check for register reference
                if next == '@' {
                    value_start = Some(i + 1);
                    break;
                }
                if next.is_ascii_digit() || next == '-' {
                    value_start = Some(i + 1);
                    break;
                }
                // Check for true/false/null
                let rest = &args[i + 1..];
                if rest.starts_with("true") || rest.starts_with("false") || rest.starts_with("null")
                {
                    value_start = Some(i + 1);
                    break;
                }
            }
        }
    }

    let value_start = value_start?;

    // Get the path (everything before value start, trimmed)
    let path = args[..value_start].trim().to_string();
    let value = args[value_start..].trim().to_string();

    if path.is_empty() || value.is_empty() {
        return None;
    }

    Some((path, value))
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
