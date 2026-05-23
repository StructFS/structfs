use std::collections::BTreeMap;

use structfs_core_store::mount_store::{MountConfig, MountStore, StoreFactory};
use structfs_core_store::overlay_store::StoreBox;
use structfs_core_store::{Error, Path, Reader, Record, Value, Writer};
use structfs_json_store::InMemoryStore;
use structfs_serde_store::{json_to_value, value_to_json};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Conditionally apply wasm_bindgen only on wasm32 targets.
#[cfg(target_arch = "wasm32")]
macro_rules! wasm_export {
    ($(#[$meta:meta])* pub fn $name:ident($($arg:ident : $ty:ty),*) $(-> $ret:ty)? $body:block) => {
        #[wasm_bindgen]
        $(#[$meta])*
        pub fn $name($($arg : $ty),*) $(-> $ret)? $body
    };
}

#[cfg(not(target_arch = "wasm32"))]
macro_rules! wasm_export {
    ($(#[$meta:meta])* pub fn $name:ident($($arg:ident : $ty:ty),*) $(-> $ret:ty)? $body:block) => {
        $(#[$meta])*
        pub fn $name($($arg : $ty),*) $(-> $ret)? $body
    };
}

/// Minimal factory for WASM — only creates memory stores.
struct WasmStoreFactory;

impl StoreFactory for WasmStoreFactory {
    fn create(&self, config: &MountConfig) -> Result<StoreBox, Error> {
        match config {
            MountConfig::Memory => Ok(Box::new(InMemoryStore::new())),
            other => Err(Error::store(
                "wasm",
                "create",
                format!("{:?} stores are not available in the playground", other),
            )),
        }
    }
}

/// Playground session state.
struct Session {
    store: MountStore<WasmStoreFactory>,
    registers: BTreeMap<String, Value>,
}

impl Session {
    fn new() -> Self {
        let mut store = MountStore::new(WasmStoreFactory);

        // Pre-mount a memory store at /data for convenience
        let _ = store.mount("data", MountConfig::Memory);

        Self {
            store,
            registers: BTreeMap::new(),
        }
    }
}

// Global session — WASM is single-threaded, no Mutex needed.
thread_local! {
    static SESSION: std::cell::RefCell<Session> = std::cell::RefCell::new(Session::new());
}

wasm_export! {
    /// Initialize the playground session.
    pub fn init() {
        SESSION.with(|s| {
            *s.borrow_mut() = Session::new();
        });
    }
}

wasm_export! {
    /// Execute a single command. Returns the output as a string.
    pub fn execute(input: &str) -> String {
        SESSION.with(|s| {
            let mut session = s.borrow_mut();
            execute_inner(input.trim(), &mut session)
        })
    }
}

fn execute_inner(input: &str, session: &mut Session) -> String {
    if input.is_empty() {
        return String::new();
    }

    // Register capture: @name command ...
    if input.starts_with('@') {
        return execute_register_capture(input, session);
    }

    execute_command(input, session)
}

fn execute_register_capture(input: &str, session: &mut Session) -> String {
    let rest = &input[1..];
    let space_pos = match rest.find(char::is_whitespace) {
        Some(pos) => pos,
        None => return format!("Error: @{} — missing command", rest),
    };

    let register_name = rest[..space_pos].to_string();
    let command = rest[space_pos..].trim();

    if register_name.is_empty() {
        return "Error: empty register name".to_string();
    }

    let output = execute_command(command, session);

    // If the command was a read that succeeded, capture the value
    if !output.starts_with("Error:") {
        // Try to parse the output as JSON and store the value
        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&output) {
            session
                .registers
                .insert(register_name.clone(), json_to_value(json_val));
        } else {
            // Store as string value
            session
                .registers
                .insert(register_name.clone(), Value::String(output.clone()));
        }
    }

    output
}

fn execute_command(input: &str, session: &mut Session) -> String {
    let (cmd, rest) = split_first_word(input);

    match cmd {
        "read" => cmd_read(rest, session),
        "write" => cmd_write(rest, session),
        "registers" => cmd_registers(session),
        "mounts" => cmd_mounts(session),
        "help" => cmd_help(),
        _ => format!("Error: unknown command '{}'. Try: read, write, registers, mounts, help", cmd),
    }
}

fn cmd_read(args: &str, session: &mut Session) -> String {
    let path_str = args.trim();
    if path_str.is_empty() {
        return "Error: read requires a path".to_string();
    }

    // Register read: read @name
    if path_str.starts_with('@') {
        let name = &path_str[1..];
        return match session.registers.get(name) {
            Some(val) => format_value(val),
            None => format!("Error: register '{}' not set", name),
        };
    }

    // Dereference: read *@name
    let resolved = match resolve_deref(path_str, session) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let path = match Path::parse(&resolved) {
        Ok(p) => p,
        Err(e) => return format!("Error: invalid path '{}': {}", resolved, e),
    };

    match session.store.read(&path) {
        Ok(Some(record)) => format_record(&record),
        Ok(None) => "null".to_string(),
        Err(e) => format!("Error: {}", e),
    }
}

fn cmd_write(args: &str, session: &mut Session) -> String {
    let args = args.trim();

    // Split path from value
    let (path_str, value_str) = match split_path_and_value(args) {
        Some(pair) => pair,
        None => return "Error: write requires a path and a value".to_string(),
    };

    // Dereference: write *@name value
    let resolved = match resolve_deref(path_str, session) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let path = match Path::parse(&resolved) {
        Ok(p) => p,
        Err(e) => return format!("Error: invalid path '{}': {}", resolved, e),
    };

    // Parse value as JSON
    let json_val: serde_json::Value = match serde_json::from_str(value_str) {
        Ok(v) => v,
        Err(e) => return format!("Error: invalid JSON: {}", e),
    };

    let value = json_to_value(json_val);

    // Ensure intermediate parent maps exist for deep paths.
    // InMemoryStore requires parents to be present, so we create
    // empty maps for any missing ancestors.
    // Skip this for ctx/mounts/* paths — MountStore handles those internally.
    let is_mount_path = path.len() >= 3 && path[0] == "ctx" && path[1] == "mounts";

    if !is_mount_path && path.len() > 1 {
        for i in 1..path.len() {
            let ancestor = path.slice(0, i);
            match session.store.read(&ancestor) {
                Ok(Some(_)) => {} // already exists
                _ => {
                    let empty_map = Record::from(Value::Map(BTreeMap::new()));
                    if let Err(e) = session.store.write(&ancestor, empty_map) {
                        return format!("Error: {}", e);
                    }
                }
            }
        }
    }

    let record = Record::from(value);

    match session.store.write(&path, record) {
        Ok(result_path) => format!("Written to: /{}", result_path),
        Err(e) => format!("Error: {}", e),
    }
}

fn cmd_registers(session: &Session) -> String {
    if session.registers.is_empty() {
        return "(no registers set)".to_string();
    }

    session
        .registers
        .iter()
        .map(|(name, val)| format!("@{} = {}", name, format_value(val)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn cmd_mounts(session: &mut Session) -> String {
    let path = Path::parse("ctx/mounts").unwrap();
    match session.store.read(&path) {
        Ok(Some(record)) => format_record(&record),
        Ok(None) => "(no mounts)".to_string(),
        Err(e) => format!("Error: {}", e),
    }
}

fn cmd_help() -> String {
    [
        "StructFS Playground",
        "",
        "Commands:",
        "  read <path>              Read a value",
        "  write <path> <json>      Write a value",
        "  @name <command>          Capture output in a register",
        "  read @name               Read a register",
        "  read *@name              Dereference register as path",
        "  registers                List all registers",
        "  mounts                   List all mounts",
        "",
        "A memory store is pre-mounted at /data.",
        "Mount more with: write /ctx/mounts/<name> {\"type\": \"memory\"}",
    ]
    .join("\n")
}

// --- Helpers ---

fn split_first_word(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(pos) => (&s[..pos], s[pos..].trim_start()),
        None => (s, ""),
    }
}

fn split_path_and_value(s: &str) -> Option<(&str, &str)> {
    if s.is_empty() {
        return None;
    }

    // Path ends at first whitespace, value is the rest
    let (path, rest) = split_first_word(s);
    if rest.is_empty() {
        return None;
    }

    Some((path, rest))
}

fn resolve_deref(path_str: &str, session: &Session) -> Result<String, String> {
    if !path_str.starts_with("*@") {
        return Ok(path_str.to_string());
    }

    let rest = &path_str[2..];
    let (name, suffix) = match rest.find('/') {
        Some(pos) => (&rest[..pos], &rest[pos..]),
        None => (rest, ""),
    };

    match session.registers.get(name) {
        Some(Value::String(s)) => Ok(format!("{}{}", s, suffix)),
        Some(val) => Ok(format!("{}{}", format_value(val), suffix)),
        None => Err(format!("Error: register '{}' not set", name)),
    }
}

fn format_value(val: &Value) -> String {
    let json = value_to_json(val.clone());
    serde_json::to_string_pretty(&json).unwrap_or_else(|_| format!("{:?}", val))
}

fn format_record(record: &Record) -> String {
    match record.as_value() {
        Some(val) => format_value(val),
        None => match record.as_bytes() {
            Some(bytes) => String::from_utf8_lossy(bytes).to_string(),
            None => "null".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(session: &mut Session, cmd: &str) -> String {
        execute_inner(cmd, session)
    }

    fn run_ok(session: &mut Session, cmd: &str) -> String {
        let result = run(session, cmd);
        assert!(
            !result.starts_with("Error:"),
            "expected success for {:?}, got: {}",
            cmd,
            result
        );
        result
    }

    // --- Playground example buttons ---
    // Each test mirrors a button from playground.njk

    #[test]
    fn example_write_user() {
        let mut s = Session::new();
        let out = run_ok(
            &mut s,
            r#"write /data/users/alice {"name": "Alice", "email": "alice@example.com"}"#,
        );
        assert!(out.starts_with("Written to:"), "got: {}", out);
    }

    #[test]
    fn example_read_user() {
        let mut s = Session::new();
        run_ok(
            &mut s,
            r#"write /data/users/alice {"name": "Alice", "email": "alice@example.com"}"#,
        );
        let out = run_ok(&mut s, "read /data/users/alice");
        assert!(out.contains("Alice"), "got: {}", out);
        assert!(out.contains("alice@example.com"), "got: {}", out);
    }

    #[test]
    fn example_mount_store() {
        let mut s = Session::new();
        let out = run_ok(&mut s, r#"write /ctx/mounts/scratch {"type": "memory"}"#);
        assert!(out.starts_with("Written to:"), "got: {}", out);

        // Verify it works
        run_ok(&mut s, r#"write /scratch/key "value""#);
        let read = run_ok(&mut s, "read /scratch/key");
        assert!(read.contains("value"), "got: {}", read);
    }

    #[test]
    fn example_list_mounts() {
        let mut s = Session::new();
        let out = run_ok(&mut s, "mounts");
        // Should list at least the pre-mounted /data store
        assert!(out.contains("data"), "got: {}", out);
    }

    #[test]
    fn example_use_register() {
        let mut s = Session::new();
        let out = run_ok(&mut s, r#"@greeting write /data/hello "world""#);
        assert!(out.starts_with("Written to:"), "got: {}", out);
    }

    #[test]
    fn example_read_register() {
        let mut s = Session::new();
        run_ok(&mut s, r#"@greeting write /data/hello "world""#);
        let out = run_ok(&mut s, "read @greeting");
        // Register should have captured the write result
        assert!(!out.is_empty(), "register should not be empty");
    }

    #[test]
    fn example_help() {
        let mut s = Session::new();
        let out = run_ok(&mut s, "help");
        assert!(out.contains("StructFS Playground"), "got: {}", out);
        assert!(out.contains("read"), "got: {}", out);
        assert!(out.contains("write"), "got: {}", out);
    }

    // --- Core operations ---

    #[test]
    fn write_then_read_roundtrip() {
        let mut s = Session::new();
        run_ok(&mut s, r#"write /data/x 42"#);
        let out = run_ok(&mut s, "read /data/x");
        assert_eq!(out.trim(), "42");
    }

    #[test]
    fn read_nonexistent_returns_null() {
        let mut s = Session::new();
        let out = run_ok(&mut s, "read /data/nonexistent");
        assert_eq!(out.trim(), "null");
    }

    #[test]
    fn write_requires_path_and_value() {
        let mut s = Session::new();
        let out = run(&mut s, "write");
        assert!(out.starts_with("Error:"), "got: {}", out);

        let out = run(&mut s, "write /data/x");
        assert!(out.starts_with("Error:"), "got: {}", out);
    }

    #[test]
    fn read_requires_path() {
        let mut s = Session::new();
        let out = run(&mut s, "read");
        assert!(out.starts_with("Error:"), "got: {}", out);
    }

    #[test]
    fn unknown_command() {
        let mut s = Session::new();
        let out = run(&mut s, "delete /data/x");
        assert!(out.starts_with("Error:"), "got: {}", out);
        assert!(out.contains("unknown command"), "got: {}", out);
    }

    #[test]
    fn empty_input() {
        let mut s = Session::new();
        let out = run(&mut s, "");
        assert_eq!(out, "");
    }

    #[test]
    fn register_not_set() {
        let mut s = Session::new();
        let out = run(&mut s, "read @nosuch");
        assert!(out.starts_with("Error:"), "got: {}", out);
    }

    #[test]
    fn registers_empty() {
        let mut s = Session::new();
        let out = run_ok(&mut s, "registers");
        assert!(out.contains("no registers"), "got: {}", out);
    }

    #[test]
    fn registers_after_capture() {
        let mut s = Session::new();
        run_ok(&mut s, r#"@x write /data/a "hello""#);
        let out = run_ok(&mut s, "registers");
        assert!(out.contains("@x"), "got: {}", out);
    }

    #[test]
    fn nested_write_read() {
        let mut s = Session::new();
        run_ok(&mut s, r#"write /data/a/b/c {"deep": true}"#);
        let out = run_ok(&mut s, "read /data/a/b/c");
        assert!(out.contains("deep"), "got: {}", out);
    }

    #[test]
    fn invalid_json_value() {
        let mut s = Session::new();
        let out = run(&mut s, "write /data/x {not json}");
        assert!(out.starts_with("Error:"), "got: {}", out);
        assert!(out.contains("invalid JSON"), "got: {}", out);
    }
}
