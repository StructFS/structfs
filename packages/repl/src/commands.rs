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
    /// Command succeeded, optionally with output to display and a value to capture
    Ok {
        display: Option<String>,
        /// The actual value to capture in a register (if different from parsed display)
        capture: Option<JsonValue>,
    },
    /// Command failed with an error message
    Error(String),
    /// User requested to exit
    Exit,
    /// Show help
    Help,
}

impl CommandResult {
    /// Create a simple Ok result with display text
    fn ok_display(display: impl Into<String>) -> Self {
        CommandResult::Ok {
            display: Some(display.into()),
            capture: None,
        }
    }

    /// Create an Ok result with both display and capture value
    fn ok_with_capture(display: impl Into<String>, capture: JsonValue) -> Self {
        CommandResult::Ok {
            display: Some(display.into()),
            capture: Some(capture),
        }
    }

    /// Create an Ok result with no output
    fn ok_none() -> Self {
        CommandResult::Ok {
            display: None,
            capture: None,
        }
    }
}

/// Parse and execute a command
pub fn execute(input: &str, ctx: &mut StoreContext) -> CommandResult {
    let input = input.trim();

    if input.is_empty() {
        return CommandResult::ok_none();
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
        CommandResult::Ok { display, capture } => {
            // Prefer explicit capture value, otherwise try to parse display as JSON
            let (value, is_string) = if let Some(cap) = capture {
                (cap, false)
            } else if let Some(ref output) = display {
                let plain_output = strip_ansi_codes(output);
                match serde_json::from_str::<JsonValue>(&plain_output) {
                    Ok(v) => (v, false),
                    Err(_) => (JsonValue::String(plain_output), true),
                }
            } else {
                (JsonValue::Null, false)
            };

            ctx.set_register(register_name, value.clone());

            let type_hint = if value.is_null() {
                Some("(null)")
            } else if is_string {
                Some("(stored as string)")
            } else {
                None
            };

            let msg = if let Some(hint) = type_hint {
                format!(
                    "{} {} {}",
                    Color::Magenta.paint("→"),
                    Color::Magenta.paint(format!("@{}", register_name)),
                    Color::DarkGray.paint(hint)
                )
            } else {
                format!(
                    "{} {}",
                    Color::Magenta.paint("→"),
                    Color::Magenta.paint(format!("@{}", register_name))
                )
            };

            CommandResult::ok_display(msg)
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

/// Check if a path string contains a dereference (*@)
fn contains_dereference(path_str: &str) -> bool {
    path_str.contains("*@")
}

/// Resolve all dereferences (*@register) in a path string.
/// Supports patterns like:
/// - `*@handle` - simple dereference
/// - `*@handle/subpath` - dereference with suffix
/// - `/prefix/*@handle` - prefix with dereference
/// - `/prefix/*@handle/suffix` - prefix, dereference, and suffix
///
/// Returns the resolved path string, or an error message.
fn resolve_dereference(path_str: &str, ctx: &mut StoreContext) -> Result<String, String> {
    if !contains_dereference(path_str) {
        return Ok(path_str.to_string());
    }

    let mut result = String::new();
    let mut remaining = path_str;

    while let Some(deref_pos) = remaining.find("*@") {
        // Add everything before the dereference
        result.push_str(&remaining[..deref_pos]);

        // Skip past "*@"
        let after_star_at = &remaining[deref_pos + 2..];

        // Find the end of the register name (next / or end of string)
        let name_end = after_star_at.find('/').unwrap_or(after_star_at.len());

        let register_name = &after_star_at[..name_end];

        if register_name.is_empty() {
            return Err("Invalid dereference: empty register name after *@".to_string());
        }

        // Read the register value
        let reg_path = format!("@{}", register_name);
        let value = ctx
            .read_register(&reg_path)
            .map_err(|e| format!("Failed to read register '{}': {}", register_name, e))?
            .ok_or_else(|| format!("Register '{}' does not exist", register_name))?;

        // Extract the path string from the register value
        let deref_value = match value {
            JsonValue::String(s) => s,
            _ => {
                return Err(format!(
                    "Register '{}' does not contain a path string",
                    register_name
                ))
            }
        };

        // Add the dereferenced value
        result.push_str(&deref_value);

        // Continue with the rest of the string
        remaining = &after_star_at[name_end..];
    }

    // Add any remaining content after the last dereference
    result.push_str(remaining);

    Ok(result)
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
        "  Dereference register:     {}\n",
        arg_style.paint("read *@handle  or  write *@handle \"data\"")
    ));
    help.push_str(&format!(
        "  Dereference with suffix:  {}\n",
        arg_style.paint("read *@handle/meta")
    ));

    help.push_str(&format!(
        "\n{}",
        Style::new()
            .italic()
            .paint("Paths: Use '/' for root, '..' to go up, '@name' for registers, '*@name' to dereference")
    ));

    help
}

fn cmd_read(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "." } else { args };

    // Resolve any dereferences (*@register) in the path
    let path_str = match resolve_dereference(path_str, ctx) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(e),
    };

    // Check if this is a register path
    if StoreContext::is_register_path(&path_str) {
        match ctx.read_register(&path_str) {
            Ok(Some(value)) => {
                let mut output = format_json(&value);

                // Add dereference hint if the value looks like a path
                if let JsonValue::String(s) = &value {
                    if s.starts_with('/') || s.contains('/') {
                        output.push_str(&format!(
                            "\n{}",
                            Color::Cyan
                                .dimmed()
                                .paint(format!("(use *{} to dereference)", path_str))
                        ));
                    }
                }

                return CommandResult::ok_with_capture(output, value);
            }
            Ok(None) => {
                return CommandResult::ok_with_capture(
                    format!("{}", Color::Yellow.paint("null (register does not exist)")),
                    JsonValue::Null,
                )
            }
            Err(e) => return CommandResult::Error(format!("Read error: {}", e)),
        }
    }

    let path = match ctx.resolve_path(&path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
    };

    match ctx.read(&path) {
        Ok(Some(value)) => CommandResult::ok_with_capture(format_json(&value), value),
        Ok(None) => CommandResult::ok_with_capture(
            format!(
                "{}",
                Color::Yellow.paint("null (path does not exist or no store mounted)")
            ),
            JsonValue::Null,
        ),
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

    // Resolve any dereferences (*@register) in the path
    let path_str = match resolve_dereference(&path_str, ctx) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(e),
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
            Ok(result_path) => {
                let path_string = format_path(&result_path);
                return CommandResult::ok_with_capture(
                    format!(
                        "{} {}",
                        Color::Green.paint("ok"),
                        Color::Magenta.paint(&path_str)
                    ),
                    JsonValue::String(path_string),
                );
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

            // The capture value is the result path as a string
            let path_string = format_path(&full_path);

            // Check if the result path differs from the write destination
            // (indicates a broker-style store that returns a handle)
            let output = if full_path != path && !result_path.is_empty() {
                format!(
                    "{}\n{} {}",
                    Color::Green.paint("ok"),
                    Color::Cyan.paint("result path:"),
                    path_string
                )
            } else {
                format!("{}", Color::Green.paint("ok"))
            };
            CommandResult::ok_with_capture(output, JsonValue::String(path_string))
        }
        Err(e) => CommandResult::Error(format!("Write error: {}", e)),
    }
}

fn cmd_cd(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "/" } else { args };

    // Resolve any dereferences (*@register) in the path
    let path_str = match resolve_dereference(path_str, ctx) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(e),
    };

    let path = match ctx.resolve_path(&path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
    };

    ctx.set_current_path(path);
    CommandResult::ok_none()
}

fn cmd_pwd(ctx: &mut StoreContext) -> CommandResult {
    let path_str = format_path(ctx.current_path());
    CommandResult::ok_with_capture(&path_str, JsonValue::String(path_str.clone()))
}

fn cmd_mounts(ctx: &mut StoreContext) -> CommandResult {
    let mounts = ctx.list_mounts();
    if mounts.is_empty() {
        CommandResult::ok_display(format!(
            "{}",
            Color::Yellow.paint(
                "No mounts. Use 'write /ctx/mounts/<name> {\"type\": \"memory\"}' to mount a store."
            )
        ))
    } else {
        let mut output = String::new();
        for mount in &mounts {
            output.push_str(&format!(
                "  {} -> {:?}\n",
                Color::Cyan.paint(&mount.path),
                mount.config
            ));
        }
        // Capture the mounts as JSON
        let capture = serde_json::to_value(&mounts).unwrap_or(JsonValue::Null);
        CommandResult::ok_with_capture(output.trim_end().to_string(), capture)
    }
}

fn cmd_registers(ctx: &mut StoreContext) -> CommandResult {
    let registers = ctx.list_registers();
    if registers.is_empty() {
        CommandResult::ok_display(format!(
            "{}",
            Color::Yellow
                .paint("No registers. Use '@name read <path>' to store output in a register.")
        ))
    } else {
        let mut output = String::new();
        let names: Vec<String> = registers.iter().map(|s| (*s).clone()).collect();
        for name in &names {
            output.push_str(&format!(
                "  {}\n",
                Color::Magenta.paint(format!("@{}", name))
            ));
        }
        CommandResult::ok_with_capture(
            output.trim_end().to_string(),
            JsonValue::Array(names.into_iter().map(JsonValue::String).collect()),
        )
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
