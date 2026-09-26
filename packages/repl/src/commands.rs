//! REPL command parsing and execution.
//!
//! Commands are named in [`crate::command_table::COMMANDS`]; this module
//! dispatches them against a [`StoreContext`].

use nu_ansi_term::Color;
use serde_json::Value as JsonValue;

use structfs_core_store::{path, Path, Value};
use structfs_serde_store::{json_to_value, value_to_json};

use crate::command_table;
use crate::help_format::{format_help_root, format_help_value_with_path};
use crate::store_context::{is_register_path, register_store_path, StoreContext};

pub use crate::help_format::format_help;

/// Result of executing a command
#[derive(Debug)]
#[non_exhaustive]
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
    command_table::lookup(first_word)
        .filter(|command| command.capturable)
        .map(|_| (register_name.to_string(), remaining))
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

            if let Err(e) = ctx.set_register(register_name, value.clone()) {
                return CommandResult::Error(format!("Failed to store @{}: {}", register_name, e));
            }

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
                    Color::White.dimmed().paint(hint)
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

pub(crate) fn strip_ansi_codes(s: &str) -> String {
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

    let Some(spec) = command_table::lookup(command) else {
        return CommandResult::Error(format!(
            "Unknown command: '{}'. Type 'help' for available commands.",
            command
        ));
    };
    match spec.name {
        "help" => cmd_help(args, ctx),
        "exit" => CommandResult::Exit,
        "read" => cmd_read(args, ctx),
        "write" => cmd_write(args, ctx),
        "ls" => cmd_ls(args, ctx),
        "cd" => cmd_cd(args, ctx),
        "pwd" => cmd_pwd(ctx),
        "mounts" => cmd_mounts(ctx),
        "registers" => cmd_registers(ctx),
        other => CommandResult::Error(format!("command '{other}' is not dispatched")),
    }
}

/// Resolve a command's path argument: dereference `*@reg`, map `@reg` to
/// its register path, and resolve relative paths against the current path.
fn resolve_arg_path(arg: &str, ctx: &mut StoreContext) -> Result<Path, String> {
    let path_str = resolve_dereference(arg, ctx)?;
    if is_register_path(&path_str) {
        return register_store_path(&path_str).map_err(|e| format!("Invalid path: {}", e));
    }
    ctx.resolve_path(&path_str)
        .map_err(|e| format!("Invalid path: {}", e))
}

fn cmd_ls(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path = match resolve_arg_path(if args.is_empty() { "." } else { args }, ctx) {
        Ok(path) => path,
        Err(e) => return CommandResult::Error(e),
    };
    match ctx.read_children(&path) {
        Ok(Some(names)) => {
            let display = if names.is_empty() {
                format!("{}", Color::Yellow.paint("(no children)"))
            } else {
                names
                    .iter()
                    .map(|name| format!("  {}", Color::Cyan.paint(name)))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            CommandResult::ok_with_capture(
                display,
                Value::Array(names.into_iter().map(Value::String).collect()),
            )
        }
        Ok(None) => CommandResult::ok_with_capture(
            format!("{}", Color::Yellow.paint("null (path does not exist)")),
            Value::Null,
        ),
        Err(e) => CommandResult::Error(format!("ls error: {}", e)),
    }
}

fn cmd_mounts(ctx: &mut StoreContext) -> CommandResult {
    let listing = match ctx.read(&path!("ctx/mounts")) {
        Ok(Some(listing)) => listing,
        Ok(None) => Value::Array(Vec::new()),
        Err(e) => return CommandResult::Error(format!("Read error: {}", e)),
    };
    let Value::Array(entries) = &listing else {
        return CommandResult::ok_with_capture(format_ir_value(&listing), listing);
    };
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        let Value::Map(entry) = entry else { continue };
        let name = match entry.get("path") {
            Some(Value::String(name)) => name.as_str(),
            _ => "?",
        };
        let config = match entry.get("config") {
            None | Some(Value::Null) => Color::White.dimmed().paint("(built in)").to_string(),
            Some(config) => value_to_json(config.clone())
                .map(|json| json.to_string())
                .unwrap_or_else(|_| format_ir_value(config)),
        };
        lines.push(format!(
            "  {:<24} {}",
            Color::Yellow.paint(format!("/{name}")),
            config
        ));
    }
    let display = if lines.is_empty() {
        format!("{}", Color::Yellow.paint("No mounts."))
    } else {
        lines.join("\n")
    };
    CommandResult::ok_with_capture(display, listing)
}

fn cmd_read(args: &str, ctx: &mut StoreContext) -> CommandResult {
    let path_str = if args.is_empty() { "." } else { args };

    let path_str = match resolve_dereference(path_str, ctx) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(e),
    };

    if is_register_path(&path_str) {
        match ctx.read_register(&path_str) {
            Ok(Some(value)) => {
                let mut output = format_ir_value(&value);

                if let Value::String(s) = &value {
                    if s.contains('/') {
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
        Ok(Some(value)) => CommandResult::ok_with_capture(format_ir_value(&value), value),
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
    let value: Value = if let Some(register_name) = value_str.strip_prefix('@') {
        match ctx.read_register(&value_str) {
            Ok(Some(v)) => v,
            Ok(None) => {
                return CommandResult::Error(format!("Register '{}' does not exist", register_name))
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
    if is_register_path(&path_str) {
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
            // OverlayStore already returns the full path with mount prefix
            let path_string = format_path(&result_path);

            let output = if result_path != path && !result_path.is_empty() {
                format!(
                    "{}\n{} {}",
                    Color::Green.paint("ok"),
                    Color::Cyan.paint("→"),
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
    let names = ctx.list_registers();
    if names.is_empty() {
        CommandResult::ok_display(format!(
            "{}",
            Color::Yellow
                .paint("No registers. Use '@name read <path>' to store output in a register.")
        ))
    } else {
        let output = names
            .iter()
            .map(|name| format!("  {}", Color::Magenta.paint(format!("@{}", name))))
            .collect::<Vec<_>>()
            .join("\n");
        CommandResult::ok_with_capture(
            output,
            Value::Array(names.into_iter().map(Value::String).collect()),
        )
    }
}

fn cmd_help(args: &str, ctx: &mut StoreContext) -> CommandResult {
    // Build the help path: /ctx/help or /ctx/help/<topic>
    let help_path = if args.is_empty() {
        "ctx/help".to_string()
    } else {
        // Support both "help http" and "help /ctx/http" styles
        let topic = args.trim_start_matches('/');
        format!("ctx/help/{}", topic)
    };

    let path = match Path::parse(&help_path) {
        Ok(p) => p,
        Err(e) => return CommandResult::Error(format!("Invalid help path: {}", e)),
    };

    let display_path = format!("/{}", help_path);

    match ctx.read(&path) {
        Ok(Some(value)) => {
            // When showing root help (topic list), also include the built-in help
            if args.is_empty() {
                let formatted = format_help_root(&value);
                CommandResult::ok_with_capture(formatted, value)
            } else {
                let formatted = format_help_value_with_path(&value, &display_path);
                CommandResult::ok_with_capture(formatted, value)
            }
        }
        Ok(None) => {
            let suggestion = if args.is_empty() {
                "The help system is not available.".to_string()
            } else {
                format!(
                    "No help found for '{}'. Try 'help' for available topics.",
                    args
                )
            };
            CommandResult::ok_display(format!("{}", Color::Yellow.paint(suggestion)))
        }
        Err(e) => CommandResult::Error(format!("Help error: {}", e)),
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
        format!("/{}", path)
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

/// Display a full IR value without interpreting Bytes as ordinary strings.
pub(crate) fn format_ir_value(value: &Value) -> String {
    match value_to_json(value.clone()) {
        Ok(json) => format_json(&json),
        Err(_) => match structfs_core_store::Codec::encode(
            &structfs_serde_store::ValueJsonCodec,
            value,
            &structfs_core_store::Format::VALUE_JSON,
        ) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => format!("<value display failed: {error}>"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_table::COMMANDS;
    use crate::mounts::MountConfig;

    fn display_of(result: CommandResult) -> String {
        match result {
            CommandResult::Ok {
                display: Some(text),
                ..
            } => strip_ansi_codes(&text),
            other => panic!("expected Ok with display, got {other:?}"),
        }
    }

    fn capture_of(result: CommandResult) -> Value {
        match result {
            CommandResult::Ok {
                capture: Some(value),
                ..
            } => value,
            other => panic!("expected Ok with capture, got {other:?}"),
        }
    }

    #[test]
    fn every_table_entry_dispatches() {
        for spec in COMMANDS {
            for spelling in spec.spellings() {
                let mut ctx = StoreContext::new();
                let result = execute(spelling, &mut ctx);
                if let CommandResult::Error(msg) = &result {
                    assert!(
                        !msg.contains("Unknown command") && !msg.contains("not dispatched"),
                        "'{spelling}' does not dispatch: {msg}"
                    );
                }
                let captured = parse_register_capture(&format!("@r {spelling}")).is_some();
                assert_eq!(captured, spec.capturable, "{spelling}");
            }
        }
    }

    // Pure function tests
    #[test]
    fn strip_ansi_codes_no_codes() {
        assert_eq!(strip_ansi_codes("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_codes_with_codes() {
        assert_eq!(strip_ansi_codes("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_ansi_codes_multiple_codes() {
        assert_eq!(
            strip_ansi_codes("\x1b[1m\x1b[32mbold green\x1b[0m"),
            "bold green"
        );
    }

    #[test]
    fn strip_ansi_codes_empty() {
        assert_eq!(strip_ansi_codes(""), "");
    }

    #[test]
    fn contains_dereference_true() {
        assert!(contains_dereference("*@foo"));
        assert!(contains_dereference("/path/*@reg/more"));
        assert!(contains_dereference("prefix*@suffix"));
    }

    #[test]
    fn contains_dereference_false() {
        assert!(!contains_dereference("@foo"));
        assert!(!contains_dereference("/path/reg"));
        assert!(!contains_dereference("*foo"));
        assert!(!contains_dereference(""));
    }

    #[test]
    fn format_json_scalars_and_containers() {
        for (json, text) in [
            (serde_json::Value::Null, "null"),
            (serde_json::json!(true), "true"),
            (serde_json::json!(false), "false"),
            (serde_json::json!(42), "42"),
            (serde_json::json!(-42), "-42"),
            (serde_json::json!(2.75), "2.75"),
            (serde_json::json!("hello"), "hello"),
            (serde_json::json!("hello\\nworld"), "hello"),
        ] {
            assert!(
                strip_ansi_codes(&format_json(&json)).contains(text),
                "{json}"
            );
        }
        let plain = strip_ansi_codes(&format_json(
            &serde_json::json!({"outer": {"inner": [1, 2, 3]}}),
        ));
        for text in ["outer", "inner", "1", "2", "3"] {
            assert!(plain.contains(text));
        }
    }

    #[test]
    fn format_ir_value_shows_bytes() {
        let shown = format_ir_value(&Value::Bytes(vec![1, 2, 3]));
        assert!(!shown.is_empty());
    }

    // Command execution tests with context
    #[test]
    fn execute_empty_input() {
        let mut ctx = StoreContext::new();
        for input in ["", "   "] {
            match execute(input, &mut ctx) {
                CommandResult::Ok { display, .. } => assert!(display.is_none()),
                _ => panic!("Expected Ok"),
            }
        }
    }

    #[test]
    fn execute_help_command() {
        let mut ctx = StoreContext::new();
        let output = display_of(execute("help", &mut ctx));
        assert!(
            output.contains("read"),
            "Expected built-in help with 'read'"
        );
        assert!(
            output.contains("write"),
            "Expected built-in help with 'write'"
        );
        assert!(output.contains("Available Help Topics"));
        assert!(output.contains("ctx/sys"), "Expected ctx/sys topic");
        assert!(output.contains("ctx/repl"), "Expected ctx/repl topic");

        assert!(!display_of(execute("?", &mut ctx)).is_empty());
    }

    #[test]
    fn execute_help_topic_command() {
        let mut ctx = StoreContext::new();
        let output = display_of(execute("help ctx/sys", &mut ctx));
        assert!(output.contains("System Primitives"), "{output}");
        let missing = display_of(execute("help nothing/here", &mut ctx));
        assert!(missing.contains("No help found"), "{missing}");
    }

    #[test]
    fn execute_exit_command() {
        let mut ctx = StoreContext::new();
        assert!(matches!(execute("exit", &mut ctx), CommandResult::Exit));
        assert!(matches!(execute("quit", &mut ctx), CommandResult::Exit));
        assert!(matches!(execute("q", &mut ctx), CommandResult::Exit));
    }

    #[test]
    fn execute_unknown_command() {
        let mut ctx = StoreContext::new();
        match execute("unknown_cmd", &mut ctx) {
            CommandResult::Error(msg) => assert!(msg.contains("Unknown command")),
            _ => panic!("Expected Error"),
        }
    }

    #[test]
    fn execute_pwd() {
        let mut ctx = StoreContext::new();
        assert_eq!(capture_of(execute("pwd", &mut ctx)), Value::from("/"));
    }

    #[test]
    fn execute_cd() {
        let mut ctx = StoreContext::new();
        execute("cd foo", &mut ctx);
        assert_eq!(ctx.current_path().to_string(), "foo");
        execute("cd /", &mut ctx);
        assert!(ctx.current_path().is_empty());
        ctx.set_current_path(path!("foo/bar"));
        execute("cd", &mut ctx);
        assert!(ctx.current_path().is_empty());
    }

    #[test]
    fn execute_registers() {
        let mut ctx = StoreContext::new();
        assert!(display_of(execute("registers", &mut ctx)).contains("No registers"));
        ctx.set_register("foo", Value::Integer(42)).unwrap();
        assert_eq!(
            capture_of(execute("regs", &mut ctx)),
            Value::Array(vec![Value::from("foo")])
        );
    }

    #[test]
    fn execute_read_sys_time() {
        let mut ctx = StoreContext::new();
        assert!(matches!(
            capture_of(execute("read /ctx/sys/time/now", &mut ctx)),
            Value::String(_)
        ));
    }

    #[test]
    fn execute_read_nonexistent() {
        let mut ctx = StoreContext::new();
        ctx.mount("test", MountConfig::Memory).unwrap();
        assert!(display_of(execute("read /test/nonexistent", &mut ctx)).contains("null"));
    }

    #[test]
    fn execute_read_register() {
        let mut ctx = StoreContext::new();
        ctx.set_register("foo", Value::from("bar/baz")).unwrap();
        let text = display_of(execute("read @foo", &mut ctx));
        assert!(text.contains("bar") && text.contains("*@foo"), "{text}");
        assert!(display_of(execute("read @nonexistent", &mut ctx)).contains("null"));
    }

    #[test]
    fn execute_write_to_memory() {
        let mut ctx = StoreContext::new();
        ctx.mount("test", MountConfig::Memory).unwrap();
        assert!(display_of(execute("write /test/key 42", &mut ctx)).contains("ok"));
        assert!(matches!(
            execute("write /test/data {\"key\": \"value\"}", &mut ctx),
            CommandResult::Ok { .. }
        ));
        assert!(matches!(
            execute("write /test/data {invalid}", &mut ctx),
            CommandResult::Error(_)
        ));
        assert!(matches!(
            execute("write /test/path", &mut ctx),
            CommandResult::Error(_)
        ));
    }

    #[test]
    fn execute_register_capture() {
        let mut ctx = StoreContext::new();
        assert!(matches!(
            execute("@result pwd", &mut ctx),
            CommandResult::Ok { .. }
        ));
        assert!(ctx.get_register("result").unwrap().is_some());
        execute("@time read /ctx/sys/time/now_unix", &mut ctx);
        assert!(ctx.get_register("time").unwrap().is_some());
    }

    #[test]
    fn execute_dereference() {
        let mut ctx = StoreContext::new();
        ctx.set_register("path", Value::String("/ctx/sys/time/now".to_string()))
            .unwrap();
        assert!(matches!(
            execute("read *@path", &mut ctx),
            CommandResult::Ok { .. }
        ));
        assert!(matches!(
            execute("read *@nonexistent", &mut ctx),
            CommandResult::Error(_)
        ));
    }

    #[test]
    fn execute_write_to_register() {
        let mut ctx = StoreContext::new();
        assert!(matches!(
            execute("write @myreg 42", &mut ctx),
            CommandResult::Ok { .. }
        ));
        assert_eq!(ctx.get_register("myreg").unwrap(), Some(Value::Integer(42)));
    }

    #[test]
    fn nested_register_write_sets_a_child_instead_of_replacing() {
        let mut ctx = StoreContext::new();
        execute("write @foo {\"keep\": 1}", &mut ctx);
        assert!(matches!(
            execute("write @foo/bar 2", &mut ctx),
            CommandResult::Ok { .. }
        ));
        let foo = ctx.get_register("foo").unwrap().unwrap();
        assert_eq!(foo.get(&path!("keep")), Some(&Value::Integer(1)), "{foo:?}");
        assert_eq!(foo.get(&path!("bar")), Some(&Value::Integer(2)));
        // A child of a scalar cannot be set.
        execute("write @n 1", &mut ctx);
        assert!(matches!(
            execute("write @n/x 2", &mut ctx),
            CommandResult::Error(_)
        ));
    }

    #[test]
    fn execute_write_from_register() {
        let mut ctx = StoreContext::new();
        ctx.set_register("source", Value::Integer(99)).unwrap();
        ctx.mount("test", MountConfig::Memory).unwrap();
        assert!(matches!(
            execute("write /test/dest @source", &mut ctx),
            CommandResult::Ok { .. }
        ));
        assert_eq!(
            ctx.read(&path!("test/dest")).unwrap(),
            Some(Value::Integer(99))
        );
    }

    #[test]
    fn mounts_lists_every_mount_with_its_config() {
        let mut ctx = StoreContext::new();
        ctx.mount("data", MountConfig::Memory).unwrap();
        let result = execute("mounts", &mut ctx);
        let CommandResult::Ok {
            display: Some(display),
            capture: Some(Value::Array(entries)),
        } = result
        else {
            panic!("expected a listing");
        };
        let display = strip_ansi_codes(&display);
        assert!(display.contains("/data"), "{display}");
        assert!(display.contains("\"type\":\"memory\""), "{display}");
        assert!(display.contains("/ctx/help") && display.contains("(built in)"));
        assert_eq!(entries.len(), ctx.mount_count());

        let mut empty = StoreContext::with_factory(crate::mounts::CoreReplStoreFactory);
        assert!(display_of(execute("mounts", &mut empty)).contains("No mounts"));
    }

    #[test]
    fn ls_lists_children() {
        let mut ctx = StoreContext::new();
        ctx.mount("data", MountConfig::Memory).unwrap();
        execute("write /data/users/alice 1", &mut ctx);
        execute("write /data/users/bob 2", &mut ctx);

        assert_eq!(
            capture_of(execute("ls /data/users", &mut ctx)),
            Value::Array(vec![Value::from("alice"), Value::from("bob")])
        );
        // Relative to the current path, and between mounts.
        execute("cd /data", &mut ctx);
        assert_eq!(
            capture_of(execute("ls", &mut ctx)),
            Value::Array(vec![Value::from("users")])
        );
        assert!(display_of(execute("ls /", &mut ctx)).contains("data"));
        let ctx_listing = display_of(execute("ls /ctx", &mut ctx));
        assert!(
            ctx_listing.contains("sys") && ctx_listing.contains("mounts"),
            "{ctx_listing}"
        );
        // Missing paths, registers, dereferences, and errors.
        assert_eq!(
            capture_of(execute("ls /data/nobody", &mut ctx)),
            Value::Null
        );
        ctx.set_register("m", Value::Map(Default::default()))
            .unwrap();
        execute("write @m/k 1", &mut ctx);
        assert_eq!(
            capture_of(execute("ls @m", &mut ctx)),
            Value::Array(vec![Value::from("k")])
        );
        ctx.set_register("where", Value::from("/data")).unwrap();
        assert_eq!(
            capture_of(execute("ls *@where", &mut ctx)),
            Value::Array(vec![Value::from("users")])
        );
        assert!(matches!(
            execute("ls /nowhere", &mut ctx),
            CommandResult::Error(_)
        ));
        assert!(matches!(
            execute("ls *@missing", &mut ctx),
            CommandResult::Error(_)
        ));
        // Capturable.
        execute("@kids ls /data/users", &mut ctx);
        assert_eq!(
            ctx.get_register("kids").unwrap(),
            Some(Value::Array(vec![Value::from("alice"), Value::from("bob")]))
        );
    }

    #[test]
    fn ls_of_listings_shows_names_not_indices() {
        let mut ctx = StoreContext::new();
        ctx.set_register("alpha", Value::Integer(1)).unwrap();
        ctx.set_register("beta", Value::Integer(2)).unwrap();
        let names = Value::Array(vec![Value::from("alpha"), Value::from("beta")]);
        assert_eq!(capture_of(execute("ls @", &mut ctx)), names);
        assert_eq!(capture_of(execute("ls /ctx/registers", &mut ctx)), names);

        let Value::Array(mounts) = capture_of(execute("ls /ctx/mounts", &mut ctx)) else {
            panic!("mount names");
        };
        assert!(mounts.contains(&Value::from("ctx/sys")), "{mounts:?}");
        assert!(!mounts.contains(&Value::from("0")));
    }

    #[test]
    fn claude_md_register_example_writes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test");
        let mut ctx = StoreContext::new();
        let open = format!(
            "@handle write /ctx/sys/fs/open {{\"path\": \"{}\", \"mode\": \"write\", \"encoding\": \"utf8\"}}",
            file.display()
        );
        for line in [open.as_str(), "write *@handle \"Hello\"", "read @handle"] {
            let result = execute(line, &mut ctx);
            assert!(
                matches!(result, CommandResult::Ok { .. }),
                "{line}: {result:?}"
            );
        }
        assert!(matches!(
            execute("write *@handle/close null", &mut ctx),
            CommandResult::Ok { .. }
        ));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Hello");
    }

    // Original tests
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

    #[test]
    fn test_parse_write_args_with_nested_json() {
        let (path, val) = parse_write_args("/data {\"nested\": {\"key\": \"value\"}}").unwrap();
        assert_eq!(path, "/data");
        assert!(val.contains("nested"));
    }

    #[test]
    fn test_parse_write_args_array() {
        let (path, val) = parse_write_args("/items [1, 2, 3]").unwrap();
        assert_eq!(path, "/items");
        assert!(val.contains('['));
    }

    #[test]
    fn test_parse_write_args_string_value() {
        let (path, val) = parse_write_args("/name \"Alice\"").unwrap();
        assert_eq!(path, "/name");
        assert_eq!(val, "\"Alice\"");
    }

    #[test]
    fn test_parse_write_args_null() {
        let (path, val) = parse_write_args("/delete null").unwrap();
        assert_eq!(path, "/delete");
        assert_eq!(val, "null");
    }

    #[test]
    fn test_parse_write_args_path_only() {
        assert!(parse_write_args("/only/path").is_none());
    }

    #[test]
    fn test_parse_register_capture_valid() {
        let (name, cmd) = parse_register_capture("@result read /foo").unwrap();
        assert_eq!(name, "result");
        assert_eq!(cmd, "read /foo");
    }

    #[test]
    fn test_parse_register_capture_write() {
        let (name, cmd) = parse_register_capture("@handle write /foo {\"x\": 1}").unwrap();
        assert_eq!(name, "handle");
        assert!(cmd.starts_with("write"));
    }

    #[test]
    fn test_parse_register_capture_rejections() {
        assert!(parse_register_capture("read /foo").is_none());
        assert!(parse_register_capture("@name").is_none());
        assert!(parse_register_capture("@ read /foo").is_none());
        assert!(parse_register_capture("@foo/bar read /baz").is_none());
        assert!(parse_register_capture("@name something /foo").is_none());
        assert!(parse_register_capture("@name help").is_none());
    }

    #[test]
    fn test_parse_register_capture_aliases() {
        assert!(parse_register_capture("@r get /foo").is_some());
        assert!(parse_register_capture("@r r /foo").is_some());
        assert!(parse_register_capture("@r set /foo 1").is_some());
        assert!(parse_register_capture("@r w /foo 1").is_some());
        assert!(parse_register_capture("@r cd /foo").is_some());
        assert!(parse_register_capture("@r pwd").is_some());
        assert!(parse_register_capture("@r mounts").is_some());
        assert!(parse_register_capture("@r ls /").is_some());
    }

    #[test]
    fn command_result_constructors() {
        match CommandResult::ok_display("test") {
            CommandResult::Ok { display, capture } => {
                assert_eq!(display, Some("test".to_string()));
                assert!(capture.is_none());
            }
            _ => panic!("Expected Ok variant"),
        }
        match CommandResult::ok_with_capture("test", Value::Integer(42)) {
            CommandResult::Ok { display, capture } => {
                assert_eq!(display, Some("test".to_string()));
                assert_eq!(capture, Some(Value::Integer(42)));
            }
            _ => panic!("Expected Ok variant"),
        }
        match CommandResult::ok_none() {
            CommandResult::Ok { display, capture } => {
                assert!(display.is_none());
                assert!(capture.is_none());
            }
            _ => panic!("Expected Ok variant"),
        }
    }

    #[test]
    fn format_path_shapes() {
        assert_eq!(format_path(&path!("foo/bar")), "/foo/bar");
        assert_eq!(format_path(&path!("")), "/");
    }
}
