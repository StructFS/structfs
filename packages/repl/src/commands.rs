//! REPL command parsing and execution using new architecture.
//!
//! This module mirrors commands.rs but uses the new core-store architecture
//! with Value instead of JsonValue internally.

use nu_ansi_term::{Color, Style};
use serde_json::Value as JsonValue;

use structfs_core_store::{Path, Value};
use structfs_serde_store::{json_to_value, value_to_json};

use crate::store_context::StoreContext;

/// Result of executing a command
pub enum CommandResult {
    /// Command succeeded, optionally with output to display and a value to capture
    Ok {
        display: Option<String>,
        /// The actual value to capture in a register
        capture: Option<Value>,
    },
    /// Command failed with an error message
    Error(String),
    /// User requested to exit
    Exit,
    /// Show help
    Help,
}

impl CommandResult {
    fn ok_display(display: impl Into<String>) -> Self {
        CommandResult::Ok {
            display: Some(display.into()),
            capture: None,
        }
    }

    fn ok_with_capture(display: impl Into<String>, capture: Value) -> Self {
        CommandResult::Ok {
            display: Some(display.into()),
            capture: Some(capture),
        }
    }

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

fn parse_register_capture(input: &str) -> Option<(String, &str)> {
    if !input.starts_with('@') {
        return None;
    }

    let rest = &input[1..];
    let space_pos = rest.find(char::is_whitespace)?;

    let register_name = &rest[..space_pos];
    if register_name.is_empty() || register_name.contains('/') {
        return None;
    }

    let remaining = rest[space_pos..].trim_start();
    if remaining.is_empty() {
        return None;
    }

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

fn execute_with_capture(register_name: &str, input: &str, ctx: &mut StoreContext) -> CommandResult {
    let result = execute_command(input, ctx);

    match result {
        CommandResult::Ok { display, capture } => {
            let (value, is_string) = if let Some(cap) = capture {
                (cap, false)
            } else if let Some(ref output) = display {
                let plain_output = strip_ansi_codes(output);
                match serde_json::from_str::<JsonValue>(&plain_output) {
                    Ok(v) => (json_to_value(v), false),
                    Err(_) => (Value::String(plain_output), true),
                }
            } else {
                (Value::Null, false)
            };

            ctx.set_register(register_name, value.clone());

            let type_hint = if matches!(value, Value::Null) {
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

fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
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

fn contains_dereference(path_str: &str) -> bool {
    path_str.contains("*@")
}

fn resolve_dereference(path_str: &str, ctx: &mut StoreContext) -> Result<String, String> {
    if !contains_dereference(path_str) {
        return Ok(path_str.to_string());
    }

    let mut result = String::new();
    let mut remaining = path_str;

    while let Some(deref_pos) = remaining.find("*@") {
        result.push_str(&remaining[..deref_pos]);
        let after_star_at = &remaining[deref_pos + 2..];
        let name_end = after_star_at.find('/').unwrap_or(after_star_at.len());
        let register_name = &after_star_at[..name_end];

        if register_name.is_empty() {
            return Err("Invalid dereference: empty register name after *@".to_string());
        }

        let reg_path = format!("@{}", register_name);
        let value = ctx
            .read_register(&reg_path)
            .map_err(|e| format!("Failed to read register '{}': {}", register_name, e))?
            .ok_or_else(|| format!("Register '{}' does not exist", register_name))?;

        let deref_value = match value {
            Value::String(s) => s,
            _ => {
                return Err(format!(
                    "Register '{}' does not contain a path string",
                    register_name
                ))
            }
        };

        result.push_str(&deref_value);
        remaining = &after_star_at[name_end..];
    }

    result.push_str(remaining);
    Ok(result)
}

fn execute_command(input: &str, ctx: &mut StoreContext) -> CommandResult {
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
        Style::new()
            .bold()
            .paint("StructFS REPL Commands (New Architecture)")
    ));

    let commands = [
        (
            "read",
            "[path|@reg]",
            "Read Value from path or register (alias: get, r)",
        ),
        (
            "write",
            "<path> <json|@reg>",
            "Write Value to path (alias: set, w)",
        ),
        ("cd", "<path>", "Change current path"),
        ("pwd", "", "Print current path"),
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
        Style::new().bold().paint("Default Mounts")
    ));
    help.push_str(&format!(
        "  {} - System primitives (env, time, random, proc)\n",
        arg_style.paint("/ctx/sys")
    ));
    help.push_str(&format!(
        "  {} - HTTP broker (sync)\n",
        arg_style.paint("/ctx/http_sync")
    ));

    help.push_str(&format!("\n{}\n", Style::new().bold().paint("Registers")));
    help.push_str(&format!(
        "  Store output:         {}\n",
        arg_style.paint("@result read /some/path")
    ));
    help.push_str(&format!(
        "  Read register:        {}\n",
        arg_style.paint("read @result")
    ));
    help.push_str(&format!(
        "  Dereference:          {}\n",
        arg_style.paint("read *@handle")
    ));

    help
}

fn cmd_read(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "." } else { args };

    let path_str = match resolve_dereference(path_str, ctx) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(e),
    };

    if StoreContext::is_register_path(&path_str) {
        match ctx.read_register(&path_str) {
            Ok(Some(value)) => {
                let json = value_to_json(value.clone());
                let mut output = format_json(&json);

                if let Value::String(s) = &value {
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
                    Value::Null,
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
        Ok(Some(value)) => {
            let json = value_to_json(value.clone());
            CommandResult::ok_with_capture(format_json(&json), value)
        }
        Ok(None) => CommandResult::ok_with_capture(
            format!(
                "{}",
                Color::Yellow.paint("null (path does not exist or no store mounted)")
            ),
            Value::Null,
        ),
        Err(e) => CommandResult::Error(format!("Read error: {}", e)),
    }
}

fn cmd_write(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let (path_str, value_str) =
        match parse_write_args(args) {
            Some(parts) => parts,
            None => return CommandResult::Error(
                "Usage: write <path> <json|@register>\nExample: write /data {\"name\": \"Alice\"}"
                    .to_string(),
            ),
        };

    let path_str = match resolve_dereference(&path_str, ctx) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(e),
    };

    // Get the value - either from JSON or from a register
    let value: Value = if let Some(_reg_name) = value_str.strip_prefix('@') {
        match ctx.read_register(&value_str) {
            Ok(Some(v)) => v,
            Ok(None) => {
                return CommandResult::Error(format!(
                    "Register '{}' does not exist",
                    &value_str[1..]
                ))
            }
            Err(e) => return CommandResult::Error(format!("Error reading register: {}", e)),
        }
    } else {
        match serde_json::from_str::<JsonValue>(&value_str) {
            Ok(v) => json_to_value(v),
            Err(e) => return CommandResult::Error(format!("Invalid JSON: {}", e)),
        }
    };

    // Check if destination is a register
    if StoreContext::is_register_path(&path_str) {
        match ctx.write_register(&path_str, value) {
            Ok(result_path) => {
                let path_string = format_path(&result_path);
                return CommandResult::ok_with_capture(
                    format!(
                        "{} {}",
                        Color::Green.paint("ok"),
                        Color::Magenta.paint(&path_str)
                    ),
                    Value::String(path_string),
                );
            }
            Err(e) => return CommandResult::Error(format!("Write error: {}", e)),
        }
    }

    let path = match ctx.resolve_path(&path_str) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid path: {}", e)),
    };

    match ctx.write(&path, value) {
        Ok(result_path) => {
            let full_path = if result_path.to_string().starts_with(&path.to_string()) {
                result_path.clone()
            } else {
                path.join(&result_path)
            };

            let path_string = format_path(&full_path);

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
            CommandResult::ok_with_capture(output, Value::String(path_string))
        }
        Err(e) => CommandResult::Error(format!("Write error: {}", e)),
    }
}

fn cmd_cd(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "/" } else { args };

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
    CommandResult::ok_with_capture(&path_str, Value::String(path_str.clone()))
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
            Value::Array(names.into_iter().map(Value::String).collect()),
        )
    }
}

fn parse_write_args(args: &str) -> Option<(String, String)> {
    let args = args.trim();
    if args.is_empty() {
        return None;
    }

    let mut value_start = None;

    for (i, c) in args.char_indices() {
        if c == '{' || c == '[' || c == '"' {
            value_start = Some(i);
            break;
        }
    }

    if value_start.is_none() {
        let chars: Vec<char> = args.chars().collect();
        for i in 0..chars.len().saturating_sub(1) {
            if chars[i].is_whitespace() {
                let next = chars[i + 1];
                if next == '@' {
                    value_start = Some(i + 1);
                    break;
                }
                if next.is_ascii_digit() || next == '-' {
                    value_start = Some(i + 1);
                    break;
                }
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
    let path = args[..value_start].trim().to_string();
    let value = args[value_start..].trim().to_string();

    if path.is_empty() || value.is_empty() {
        return None;
    }

    Some((path, value))
}

fn format_path(path: &Path) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", path.components.join("/"))
    }
}

fn format_json(value: &JsonValue) -> String {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());

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
    }
}
