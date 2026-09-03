//! Native blocks: Rust code run as a block, plus the builtin set.
//!
//! A native block is exactly a wasm block without the sandbox: it runs on
//! a blocking thread against its [`Namespace`], reads mailbox events from
//! `iso/server/requests`, and writes responses. The builtins (`kv`,
//! `echo`, `logger`, `shell`) double as reference implementations of the
//! block run loop.

use std::sync::Arc;

use structfs_core_store::{path, Error, MemoryStore, Path, Reader, Record, Value, Writer};

use crate::namespace::Namespace;
use crate::protocol::{error_to_response, ok_path, ok_value, EventEnvelope, RequestEnvelope};

/// A block implemented in native Rust.
pub trait NativeBlock: Send {
    /// The block's main. Runs on a blocking thread; return ends the block.
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error>;
}

/// Creates fresh instances of a native block (one per start).
pub trait NativeBlockFactory: Send + Sync {
    /// Create an instance.
    fn create(&self) -> Box<dyn NativeBlock>;
}

impl<F> NativeBlockFactory for F
where
    F: Fn() -> Box<dyn NativeBlock> + Send + Sync,
{
    fn create(&self) -> Box<dyn NativeBlock> {
        self()
    }
}

/// The canonical block run loop (`isotope/spec/07-server-protocol.md`):
/// read mailbox events until shutdown unblocks with Null, dispatch each
/// to the handler, write responses for requests, then signal shutdown
/// complete (exit code 0).
///
/// The handler receives the namespace, so a serving block can call its
/// own wired services while handling an event. Returning `Some(response)`
/// answers a request; return `None` for events (signals, timers) that
/// take no response.
pub fn serve_requests(
    ns: &mut Namespace,
    mut handler: impl FnMut(&mut Namespace, &EventEnvelope) -> Option<Value>,
) -> Result<(), Error> {
    loop {
        let Some(record) = ns.read(&path!("iso/server/requests"))? else {
            break;
        };
        let value = record.as_value().cloned().unwrap_or(Value::Null);
        let event = EventEnvelope::from_value(&value)?;
        if event == EventEnvelope::Shutdown {
            break;
        }
        let respond_to = match &event {
            EventEnvelope::Request(request) => Some(request.respond_to.clone()),
            _ => None,
        };
        let response = handler(ns, &event);
        if let (Some(respond_to), Some(response)) = (respond_to, response) {
            ns.write(&respond_to, Record::parsed(response))?;
        }
    }
    ns.write(
        &path!("iso/shutdown/complete"),
        Record::parsed(Value::map()),
    )?;
    Ok(())
}

/// Serve only server-protocol requests, ignoring other mailbox events.
pub fn serve_only_requests(
    ns: &mut Namespace,
    mut handler: impl FnMut(&mut Namespace, &RequestEnvelope) -> Value,
) -> Result<(), Error> {
    serve_requests(ns, |ns, event| match event {
        EventEnvelope::Request(request) => Some(handler(ns, request)),
        _ => None,
    })
}

// === builtin:kv ===

/// An in-memory key-value store served over the server protocol.
pub struct KvBlock {
    store: MemoryStore,
}

impl KvBlock {
    pub fn new() -> Self {
        Self {
            store: MemoryStore::new(),
        }
    }
}

impl Default for KvBlock {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeBlock for KvBlock {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        ns.write(
            &path!("iso/self/interface"),
            Record::parsed(Value::from("kv: read/write any path")),
        )?;
        let store = &mut self.store;
        serve_only_requests(ns, |_ns, request| match request.op.as_str() {
            "read" => match store.read(&request.path) {
                Ok(Some(record)) => ok_value(record.as_value().cloned().unwrap_or(Value::Null)),
                Ok(None) => ok_value(Value::Null),
                Err(e) => error_to_response(&e),
            },
            "write" => match store.write(&request.path, Record::parsed(request.data.clone())) {
                Ok(result) => ok_path(&result),
                Err(e) => error_to_response(&e),
            },
            _ => error_to_response(&Error::store("kv", "serve", "unknown op")),
        })
    }
}

// === builtin:echo ===

/// Echoes reads back and accepts writes.
pub struct EchoBlock;

impl NativeBlock for EchoBlock {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        serve_only_requests(ns, |_ns, request| match request.op.as_str() {
            "read" => {
                let mut map = std::collections::BTreeMap::new();
                map.insert("echo".to_string(), Value::String(request.path.to_string()));
                ok_value(Value::Map(map))
            }
            "write" => ok_path(&request.path),
            _ => error_to_response(&Error::store("echo", "serve", "unknown op")),
        })
    }
}

// === builtin:logger ===

/// Forwards every write to `iso/log/info` — a service block whose only
/// job is turning wired writes into log lines.
pub struct LoggerBlock;

impl NativeBlock for LoggerBlock {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        serve_only_requests(ns, |ns, request| match request.op.as_str() {
            "write" => {
                match ns.write(&path!("iso/log/info"), Record::parsed(request.data.clone())) {
                    Ok(_) => ok_path(&request.path),
                    Err(e) => error_to_response(&e),
                }
            }
            _ => ok_value(Value::from("logger: write to log")),
        })
    }
}

// === builtin:shell ===

/// An interactive shell over a block namespace.
///
/// Commands execute as store operations against the shell's own
/// namespace, exercising the whole OS surface: `/iso/` system paths and
/// every wired service. All I/O goes through `iso/stdio` — the shell has
/// no out-of-band channel to the terminal.
pub struct ShellBlock;

impl ShellBlock {
    fn read_stdin(ns: &mut Namespace) -> Result<Option<String>, Error> {
        match ns.read(&path!("iso/stdio/stdin"))? {
            Some(record) => match record.as_value() {
                Some(Value::String(line)) => Ok(Some(line.clone())),
                _ => Ok(None),
            },
            None => Ok(None),
        }
    }

    fn print(ns: &mut Namespace, text: impl AsRef<str>) {
        let _ = ns.write(
            &path!("iso/stdio/stdout"),
            Record::parsed(Value::String(format!("{}\n", text.as_ref()))),
        );
    }

    fn print_value(ns: &mut Namespace, value: &Value) {
        let json = structfs_serde_store::value_to_json(value.clone());
        match serde_json::to_string_pretty(&json) {
            Ok(text) => Self::print(ns, text),
            Err(_) => Self::print(ns, format!("{value:?}")),
        }
    }

    fn parse_path(ns: &mut Namespace, raw: &str) -> Option<Path> {
        match Path::parse(raw) {
            Ok(path) => Some(path),
            Err(e) => {
                Self::print(ns, format!("error: {e}"));
                None
            }
        }
    }

    fn read_and_print(ns: &mut Namespace, raw: &str) {
        if let Some(path) = Self::parse_path(ns, raw) {
            match ns.read(&path) {
                Ok(Some(record)) => {
                    let value = record.as_value().cloned().unwrap_or(Value::Null);
                    Self::print_value(ns, &value);
                }
                Ok(None) => Self::print(ns, "(absent)"),
                Err(e) => Self::print(ns, format!("error: {e}")),
            }
        }
    }

    /// Returns false when the shell should exit.
    fn dispatch(ns: &mut Namespace, line: &str) -> bool {
        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            return true;
        };
        let rest: Vec<&str> = parts.collect();
        match (command, rest.as_slice()) {
            ("help", _) => Self::print(ns, HELP),
            ("read", [raw]) => Self::read_and_print(ns, raw),
            ("write", [raw, json @ ..]) if !json.is_empty() => {
                if let Some(path) = Self::parse_path(ns, raw) {
                    let text = json.join(" ");
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(json) => {
                            let value = structfs_serde_store::json_to_value(json);
                            match ns.write(&path, Record::parsed(value)) {
                                Ok(result) => Self::print(ns, format!("-> {result}")),
                                Err(e) => Self::print(ns, format!("error: {e}")),
                            }
                        }
                        Err(e) => Self::print(ns, format!("error: bad JSON: {e}")),
                    }
                }
            }
            ("ls", args) => {
                let raw = args.first().copied().unwrap_or("");
                if let Some(path) = Self::parse_path(ns, raw) {
                    match ns.read_children(&path) {
                        Ok(Some(children)) if children.is_empty() => Self::print(ns, "(leaf)"),
                        Ok(Some(children)) => {
                            for child in children {
                                Self::print(ns, child);
                            }
                        }
                        Ok(None) => Self::print(ns, "(absent)"),
                        Err(e) => Self::print(ns, format!("error: {e}")),
                    }
                }
            }
            ("id", _) => Self::read_and_print(ns, "iso/self/id"),
            ("state", _) => Self::read_and_print(ns, "iso/self/state"),
            ("time", _) => Self::read_and_print(ns, "iso/time/now"),
            ("uuid", _) => Self::read_and_print(ns, "iso/random/uuid"),
            ("env", _) => Self::read_and_print(ns, "iso/env"),
            ("sleep", [ms]) => Self::read_and_print(ns, &format!("iso/time/after/{ms}")),
            ("spawn", json) if !json.is_empty() => {
                let text = json.join(" ");
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(json) => {
                        let value = structfs_serde_store::json_to_value(json);
                        match ns.write(&path!("iso/proc"), Record::parsed(value)) {
                            Ok(handle) => Self::print(ns, format!("-> {handle}")),
                            Err(e) => Self::print(ns, format!("error: {e}")),
                        }
                    }
                    Err(e) => Self::print(ns, format!("error: bad JSON: {e}")),
                }
            }
            ("log", [level, message @ ..]) if !message.is_empty() => {
                let path = format!("iso/log/{level}");
                if let Some(path) = Self::parse_path(ns, &path) {
                    if let Err(e) = ns.write(&path, Record::parsed(Value::from(message.join(" "))))
                    {
                        Self::print(ns, format!("error: {e}"));
                    }
                }
            }
            ("exit" | "quit", _) => return false,
            _ => Self::print(ns, format!("unknown command: {line} (try 'help')")),
        }
        true
    }
}

const HELP: &str = "commands:
  read <path>            read a path in this block's namespace
  write <path> <json>    write a JSON value to a path
  ls [path]              list children at a path
  id | state | env       identity, lifecycle state, environment
  time | uuid            iso/time/now, iso/random/uuid
  sleep <ms>             blocking read of iso/time/after/{ms}
  spawn <assembly-json>  write a definition to iso/proc (if granted)
  log <level> <msg...>   write to iso/log/{level}
  help | exit";

impl NativeBlock for ShellBlock {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        ns.write(
            &path!("iso/self/interface"),
            Record::parsed(Value::from("interactive shell")),
        )?;

        // Config may not be wired at all (unwired reads are denied);
        // fall back to the default prompt either way.
        let prompt = match ns.read(&path!("config/prompt")) {
            Ok(Some(record)) => match record.as_value() {
                Some(Value::String(prompt)) => prompt.clone(),
                _ => "fw> ".to_string(),
            },
            _ => "fw> ".to_string(),
        };

        Self::print(ns, "featherweight isotope shell — 'help' for commands");
        loop {
            if ns.cell().shutdown_requested() {
                break;
            }
            let _ = ns.write(
                &path!("iso/stdio/stdout"),
                Record::parsed(Value::String(prompt.clone())),
            );
            match Self::read_stdin(ns)? {
                None => break, // EOF
                Some(line) => {
                    if !Self::dispatch(ns, line.trim()) {
                        break;
                    }
                }
            }
        }
        ns.write(
            &path!("iso/shutdown/complete"),
            Record::parsed(Value::map()),
        )?;
        Ok(())
    }
}

/// Register the builtin blocks (`kv`, `echo`, `logger`, `shell`).
pub fn register_builtins(runtime: &mut crate::runtime::Runtime) {
    runtime.register_builtin(
        "kv",
        Arc::new(|| Box::new(KvBlock::new()) as Box<dyn NativeBlock>),
    );
    runtime.register_builtin(
        "echo",
        Arc::new(|| Box::new(EchoBlock) as Box<dyn NativeBlock>),
    );
    runtime.register_builtin(
        "logger",
        Arc::new(|| Box::new(LoggerBlock) as Box<dyn NativeBlock>),
    );
    runtime.register_builtin(
        "shell",
        Arc::new(|| Box::new(ShellBlock) as Box<dyn NativeBlock>),
    );
}
