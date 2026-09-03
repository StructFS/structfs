//! Native blocks: Rust code run as a block, plus the builtin set.
//!
//! A native block is exactly a wasm block without the sandbox: it runs on
//! a blocking thread against its [`Namespace`], reads server-protocol
//! requests from `iso/server/requests`, and writes responses. The builtins
//! (`kv`, `echo`, `logger`, `shell`) double as reference implementations
//! of the block run loop.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use structfs_core_store::{path, Error, MemoryStore, Path, Reader, Record, Value, Writer};

use crate::namespace::Namespace;
use crate::protocol::{error_to_response, ok_path, ok_value, RequestEnvelope};

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
/// read requests until shutdown unblocks with Null, dispatch each to the
/// handler, write responses, then signal shutdown complete.
///
/// The handler receives the namespace, so a serving block can call its
/// own wired services while handling a request.
pub fn serve_requests(
    ns: &mut Namespace,
    mut handler: impl FnMut(&mut Namespace, &RequestEnvelope) -> Value,
) -> Result<(), Error> {
    loop {
        let Some(record) = ns.read(&path!("iso/server/requests"))? else {
            break;
        };
        let request = match record.as_value() {
            Some(Value::Null) | None => break, // shutdown unblock
            Some(value) => value.clone(),
        };
        let envelope = RequestEnvelope::from_value(&request)?;
        let response = handler(ns, &envelope);
        ns.write(&envelope.respond_to, Record::parsed(response))?;
    }
    ns.write(
        &path!("iso/shutdown/complete"),
        Record::parsed(Value::map()),
    )?;
    Ok(())
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
        serve_requests(ns, |_ns, request| match request.op.as_str() {
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

/// Echoes reads back and accepts writes, logging each operation.
pub struct EchoBlock;

impl NativeBlock for EchoBlock {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        serve_requests(ns, |_ns, request| match request.op.as_str() {
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
        serve_requests(ns, |ns, request| match request.op.as_str() {
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

/// A writer usable as shell output in tests.
#[derive(Clone, Default)]
pub struct SharedOutput(pub Arc<Mutex<Vec<u8>>>);

impl SharedOutput {
    /// The captured output as a string.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).to_string()
    }
}

impl Write for SharedOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// An interactive shell over a block namespace.
///
/// Commands execute as store operations against the shell's own namespace,
/// exercising the whole OS surface: `/iso/` system paths and every wired
/// service. Terminal I/O is direct — the store model has no tty story yet.
pub struct ShellBlock {
    input: Box<dyn BufRead + Send>,
    output: Box<dyn Write + Send>,
    interactive: bool,
}

impl ShellBlock {
    /// A shell on stdin/stdout.
    pub fn stdio() -> Self {
        Self {
            input: Box::new(std::io::BufReader::new(std::io::stdin())),
            output: Box::new(std::io::stdout()),
            interactive: true,
        }
    }

    /// A shell over explicit I/O (scripted input; no prompts) — for tests
    /// and `fw run --script`.
    pub fn with_io(input: Box<dyn BufRead + Send>, output: Box<dyn Write + Send>) -> Self {
        Self {
            input,
            output,
            interactive: false,
        }
    }

    fn print(&mut self, text: impl AsRef<str>) {
        let _ = writeln!(self.output, "{}", text.as_ref());
    }

    fn print_value(&mut self, value: &Value) {
        let json = structfs_serde_store::value_to_json(value.clone());
        match serde_json::to_string_pretty(&json) {
            Ok(text) => self.print(text),
            Err(_) => self.print(format!("{value:?}")),
        }
    }

    fn read_path(&mut self, ns: &mut Namespace, raw: &str) -> Option<Path> {
        match Path::parse(raw) {
            Ok(path) => Some(path),
            Err(e) => {
                let _ = ns; // path errors don't need the namespace
                self.print(format!("error: {e}"));
                None
            }
        }
    }

    fn dispatch(&mut self, ns: &mut Namespace, line: &str) -> bool {
        let mut parts = line.split_whitespace();
        let Some(command) = parts.next() else {
            return true;
        };
        let rest: Vec<&str> = parts.collect();
        match (command, rest.as_slice()) {
            ("help", _) => {
                self.print(HELP);
            }
            ("read", [raw]) => {
                if let Some(path) = self.read_path(ns, raw) {
                    match ns.read(&path) {
                        Ok(Some(record)) => {
                            let value = record.as_value().cloned().unwrap_or(Value::Null);
                            self.print_value(&value);
                        }
                        Ok(None) => self.print("(absent)"),
                        Err(e) => self.print(format!("error: {e}")),
                    }
                }
            }
            ("write", [raw, json @ ..]) if !json.is_empty() => {
                if let Some(path) = self.read_path(ns, raw) {
                    let text = json.join(" ");
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(json) => {
                            let value = structfs_serde_store::json_to_value(json);
                            match ns.write(&path, Record::parsed(value)) {
                                Ok(result) => self.print(format!("-> {result}")),
                                Err(e) => self.print(format!("error: {e}")),
                            }
                        }
                        Err(e) => self.print(format!("error: bad JSON: {e}")),
                    }
                }
            }
            ("ls", args) => {
                let raw = args.first().copied().unwrap_or("");
                if let Some(path) = self.read_path(ns, raw) {
                    match ns.read_children(&path) {
                        Ok(Some(children)) if children.is_empty() => self.print("(leaf)"),
                        Ok(Some(children)) => {
                            for child in children {
                                self.print(child);
                            }
                        }
                        Ok(None) => self.print("(absent)"),
                        Err(e) => self.print(format!("error: {e}")),
                    }
                }
            }
            ("id", _) => self.read_and_print(ns, "iso/self/id"),
            ("state", _) => self.read_and_print(ns, "iso/self/state"),
            ("time", _) => self.read_and_print(ns, "iso/time/now"),
            ("uuid", _) => self.read_and_print(ns, "iso/random/uuid"),
            ("log", [level, message @ ..]) if !message.is_empty() => {
                let path = format!("iso/log/{level}");
                if let Some(path) = self.read_path(ns, &path) {
                    match ns.write(&path, Record::parsed(Value::from(message.join(" ")))) {
                        Ok(_) => {}
                        Err(e) => self.print(format!("error: {e}")),
                    }
                }
            }
            ("exit" | "quit", _) => return false,
            _ => self.print(format!("unknown command: {line} (try 'help')")),
        }
        true
    }

    fn read_and_print(&mut self, ns: &mut Namespace, raw: &str) {
        if let Some(path) = self.read_path(ns, raw) {
            match ns.read(&path) {
                Ok(Some(record)) => {
                    let value = record.as_value().cloned().unwrap_or(Value::Null);
                    self.print_value(&value);
                }
                Ok(None) => self.print("(absent)"),
                Err(e) => self.print(format!("error: {e}")),
            }
        }
    }
}

const HELP: &str = "commands:
  read <path>            read a path in this block's namespace
  write <path> <json>    write a JSON value to a path
  ls [path]              list children at a path
  id | state             this block's identity / lifecycle state
  time | uuid            iso/time/now, iso/random/uuid
  log <level> <msg...>   write to iso/log/{level}
  help | exit";

impl NativeBlock for ShellBlock {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        ns.write(
            &path!("iso/self/interface"),
            Record::parsed(Value::from("interactive shell")),
        )?;

        let prompt = match ns.read(&path!("config/prompt"))? {
            Some(record) => match record.as_value() {
                Some(Value::String(prompt)) => prompt.clone(),
                _ => "fw> ".to_string(),
            },
            None => "fw> ".to_string(),
        };

        if self.interactive {
            self.print("featherweight isotope shell — 'help' for commands");
        }
        loop {
            if ns.cell().shutdown_requested() {
                break;
            }
            if self.interactive {
                let _ = write!(self.output, "{prompt}");
                let _ = self.output.flush();
            }
            let mut line = String::new();
            match self.input.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    if !self.dispatch(ns, line.trim()) {
                        break;
                    }
                }
                Err(e) => {
                    self.print(format!("input error: {e}"));
                    break;
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
        Arc::new(|| Box::new(ShellBlock::stdio()) as Box<dyn NativeBlock>),
    );
}
