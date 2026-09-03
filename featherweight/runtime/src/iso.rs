//! The `/iso/` system store (`isotope/spec/04-system-paths.md` and
//! `09-posix-closure.md`).
//!
//! Every block's namespace mounts this surface at `iso/`. It is Isotope's
//! syscall interface: identity, lifecycle, time, randomness, logging,
//! stdio, timers, process control (when granted), and the server-protocol
//! mailbox — all served as reads and writes.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use structfs_core_store::{DetachedReader, DetachedWriter, Error, Path, Record, Value};

use crate::block::{BlockCell, BlockEvent};
use crate::spawn::ProcStore;
use crate::stdio::Stdio;

/// Where `iso/log/{level}` writes go.
pub trait LogSink: Send + Sync {
    /// Receive one log write from a block.
    fn log(&self, block: &str, level: &str, message: &Value);
}

/// Default sink: format to stderr.
pub struct StderrLog;

impl LogSink for StderrLog {
    fn log(&self, block: &str, level: &str, message: &Value) {
        let text = match message {
            Value::String(s) => s.clone(),
            Value::Map(map) => match map.get("msg") {
                Some(Value::String(s)) => s.clone(),
                _ => format!("{:?}", map),
            },
            other => format!("{:?}", other),
        };
        eprintln!("[{level}] {block}: {text}");
    }
}

/// Everything the `/iso/` surface of one block is built from.
pub(crate) struct IsoConfig {
    pub cell: Arc<BlockCell>,
    pub log: Arc<dyn LogSink>,
    pub stdio: Arc<dyn Stdio>,
    pub env: Arc<BTreeMap<String, String>>,
    pub args: Arc<Vec<String>>,
    /// Present only when the block's definition grants `spawn`.
    pub proc: Option<ProcStore>,
    pub handle: tokio::runtime::Handle,
}

/// The per-block `/iso/` surface. Paths are relative to the `iso` mount.
pub struct IsoSurface {
    cell: Arc<BlockCell>,
    log: Arc<dyn LogSink>,
    stdio: Arc<dyn Stdio>,
    env: Arc<BTreeMap<String, String>>,
    args: Arc<Vec<String>>,
    proc: Option<ProcStore>,
    handle: tokio::runtime::Handle,
    timers: Mutex<HashMap<u64, tokio::task::JoinHandle<()>>>,
    next_timer: AtomicU64,
}

const SECTIONS: [(&str, &str); 9] = [
    ("server", "Server protocol: the event mailbox, responses"),
    ("self", "Block identity: id, state, args, interface"),
    ("shutdown", "Lifecycle control: requested, mode, complete"),
    (
        "time",
        "Time services: now, now_unix_ns, monotonic, zone, after/{ms}",
    ),
    ("random", "Randomness: uuid, int, bytes/{n}"),
    ("log", "Logging: debug, info, warn, error"),
    ("env", "Environment variables"),
    ("stdio", "Standard streams: stdin, stdout, stderr"),
    ("timers", "Mailbox timers: write {ms, tag}"),
];

impl IsoSurface {
    pub(crate) fn new(config: IsoConfig) -> Self {
        Self {
            cell: config.cell,
            log: config.log,
            stdio: config.stdio,
            env: config.env,
            args: config.args,
            proc: config.proc,
            handle: config.handle,
            timers: Mutex::new(HashMap::new()),
            next_timer: AtomicU64::new(0),
        }
    }

    fn directory(&self) -> Value {
        let mut map: BTreeMap<String, Value> = SECTIONS
            .iter()
            .map(|(name, doc)| (name.to_string(), Value::from(*doc)))
            .collect();
        if self.proc.is_some() {
            map.insert(
                "proc".to_string(),
                Value::from("Process control: spawn/wait/kill (granted)"),
            );
        }
        Value::Map(map)
    }

    fn value_to_text(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            other => format!("{other:?}"),
        }
    }

    /// Serve a read. May park (the mailbox, stdin, `time/after`, proc
    /// waits), so this is async; the namespace bridges it onto the
    /// block's thread.
    pub async fn read(&self, path: &Path) -> Result<Option<Record>, Error> {
        if path.is_empty() {
            return Ok(Some(Record::parsed(self.directory())));
        }
        // Process control (granted capability): delegate to the handle store.
        if path[0] == "proc" {
            let Some(proc) = &self.proc else {
                return Ok(None);
            };
            let rel = path.slice(1, path.len());
            return proc.clone().read_detached(&rel).await;
        }
        let value = match (path.len(), path[0].as_str()) {
            // === server (the mailbox) ===
            (1, "server") => Some(Value::Map(BTreeMap::from([
                (
                    "requests".to_string(),
                    Value::from("read: next mailbox event (blocking)"),
                ),
                (
                    "responses".to_string(),
                    Value::from("write responses/{token}"),
                ),
            ]))),
            (2, "server") if path[1] == "requests" => {
                return match self.cell.next_event().await? {
                    Some(event) => Ok(Some(Record::parsed(event.to_value()))),
                    // Shutdown: unblock with Null (spec 07).
                    None => Ok(Some(Record::parsed(Value::Null))),
                };
            }
            (3, "server") if path[1] == "requests" && path[2] == "pending" => Some(Value::Array(
                self.cell
                    .pending_events()
                    .iter()
                    .map(BlockEvent::to_value)
                    .collect(),
            )),
            // === self ===
            (1, "self") => Some(Value::Map(BTreeMap::from([
                ("id".to_string(), Value::from(self.cell.id.as_str())),
                ("state".to_string(), Value::from(self.cell.state().as_str())),
            ]))),
            (2, "self") if path[1] == "id" => Some(Value::from(self.cell.id.as_str())),
            (2, "self") if path[1] == "state" => Some(Value::from(self.cell.state().as_str())),
            (2, "self") if path[1] == "args" => Some(Value::Array(
                self.args.iter().map(|a| Value::String(a.clone())).collect(),
            )),
            (2, "self") if path[1] == "interface" => self.cell.interface(),
            (2, "self") if path[1] == "last_error" => self.cell.last_error().map(Value::from),
            // === env ===
            (1, "env") => Some(Value::Map(
                self.env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            )),
            (2, "env") => self.env.get(&path[1]).map(|v| Value::String(v.clone())),
            // === stdio ===
            (2, "stdio") if path[1] == "stdin" => {
                // Blocks the calling (block) thread until a line or EOF.
                return Ok(self
                    .stdio
                    .read_line()
                    .map(|line| Record::parsed(Value::String(line))));
            }
            // === shutdown ===
            (2, "shutdown") if path[1] == "requested" => {
                Some(Value::Bool(self.cell.shutdown_requested()))
            }
            (2, "shutdown") if path[1] == "mode" => self
                .cell
                .shutdown_mode()
                .map(|mode| Value::from(mode.as_str())),
            // === time ===
            (2, "time") if path[1] == "now" => Some(Value::String(chrono::Utc::now().to_rfc3339())),
            (2, "time") if path[1] == "now_unix_ns" => Some(Value::Integer(
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX),
            )),
            (2, "time") if path[1] == "monotonic" => {
                Some(Value::Integer(self.cell.monotonic_nanos()))
            }
            (2, "time") if path[1] == "zone" => Some(Value::from("UTC")),
            (3, "time") if path[1] == "after" => {
                let ms: u64 = path[2]
                    .parse()
                    .map_err(|_| Error::store("iso", "time_after", "bad duration"))?;
                let sleep = tokio::time::sleep(std::time::Duration::from_millis(ms));
                tokio::pin!(sleep);
                tokio::select! {
                    _ = &mut sleep => Some(Value::Integer(ms as i64)),
                    _ = self.cell.cancel.cancelled() => {
                        return Err(Error::cancelled("sleep interrupted by shutdown"));
                    }
                }
            }
            // === random ===
            (2, "random") if path[1] == "uuid" => {
                Some(Value::String(uuid::Uuid::new_v4().to_string()))
            }
            (2, "random") if path[1] == "int" => {
                let bytes = *uuid::Uuid::new_v4().as_bytes();
                Some(Value::Integer(i64::from_le_bytes(
                    bytes[..8].try_into().unwrap(),
                )))
            }
            (3, "random") if path[1] == "bytes" => {
                let n: usize = path[2]
                    .parse()
                    .map_err(|_| Error::store("iso", "random", "bad byte count"))?;
                if n > 1 << 20 {
                    return Err(Error::resource_limit("random/bytes limited to 1MiB"));
                }
                let mut bytes = Vec::with_capacity(n);
                while bytes.len() < n {
                    bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
                }
                bytes.truncate(n);
                Some(Value::Bytes(bytes))
            }
            _ => None,
        };
        Ok(value.map(Record::parsed))
    }

    /// Serve a write. Proc writes may await; everything else is immediate.
    pub async fn write(&self, path: &Path, value: Value) -> Result<Path, Error> {
        // Process control.
        if !path.is_empty() && path[0] == "proc" {
            let Some(proc) = &self.proc else {
                return Err(Error::permission_denied(
                    "spawn capability not granted to this block",
                ));
            };
            let rel = path.slice(1, path.len());
            let result = proc
                .clone()
                .write_detached(&rel, Record::parsed(value))
                .await?;
            return Ok(Path::parse("proc").unwrap().join(&result));
        }
        let components: Vec<&str> = path.iter().map(String::as_str).collect();
        match components.as_slice() {
            ["server", "responses", token] => {
                let token: u64 = token
                    .parse()
                    .map_err(|_| Error::store("iso", "respond", "bad response token"))?;
                self.cell.respond(token, value);
                Ok(path.clone())
            }
            ["self", "interface"] => {
                self.cell.set_interface(value);
                Ok(path.clone())
            }
            ["shutdown", "complete"] => {
                let code = match &value {
                    Value::Map(map) => match map.get("code") {
                        Some(Value::Integer(code)) => *code,
                        _ => 0,
                    },
                    Value::Integer(code) => *code,
                    _ => 0,
                };
                self.cell.mark_shutdown_complete(code);
                Ok(path.clone())
            }
            ["stdio", "stdout"] => {
                self.stdio.write_out(&Self::value_to_text(&value));
                Ok(path.clone())
            }
            ["stdio", "stderr"] => {
                self.stdio.write_err(&Self::value_to_text(&value));
                Ok(path.clone())
            }
            ["timers"] => {
                let Value::Map(ref map) = value else {
                    return Err(Error::store("iso", "timers", "expected {ms, tag}"));
                };
                let ms = match map.get("ms") {
                    Some(Value::Integer(ms)) if *ms >= 0 => *ms as u64,
                    _ => return Err(Error::store("iso", "timers", "missing ms")),
                };
                let tag = map.get("tag").cloned().unwrap_or(Value::Null);
                let id = self.next_timer.fetch_add(1, Ordering::SeqCst);
                let cell = self.cell.clone();
                let task = self.handle.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    cell.deliver_timer(tag);
                });
                self.timers
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(id, task);
                Ok(Path::from_components(vec![
                    "timers".to_string(),
                    id.to_string(),
                ]))
            }
            ["timers", id] => {
                // Null-write cancels a pending timer (idempotent).
                if !value.is_null() {
                    return Err(Error::conflict("timers accept only Null (cancel)"));
                }
                if let Ok(id) = id.parse::<u64>() {
                    if let Some(task) = self
                        .timers
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id)
                    {
                        task.abort();
                    }
                }
                Ok(path.clone())
            }
            ["log", level @ ("debug" | "info" | "warn" | "error")] => {
                self.log.log(&self.cell.name, level, &value);
                Ok(path.clone())
            }
            _ => Err(Error::permission_denied(format!(
                "iso path is not writable: iso/{}",
                path
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::FailurePolicy;
    use crate::stdio::{NullStdio, ScriptedStdio};
    use structfs_core_store::path;

    fn surface_with(
        stdio: Arc<dyn Stdio>,
        env: BTreeMap<String, String>,
        args: Vec<String>,
    ) -> (Arc<BlockCell>, IsoSurface) {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        let iso = IsoSurface::new(IsoConfig {
            cell: cell.clone(),
            log: Arc::new(StderrLog),
            stdio,
            env: Arc::new(env),
            args: Arc::new(args),
            proc: None,
            handle: tokio::runtime::Handle::current(),
        });
        (cell, iso)
    }

    fn surface() -> (Arc<BlockCell>, IsoSurface) {
        surface_with(Arc::new(NullStdio), BTreeMap::new(), Vec::new())
    }

    async fn read_value(iso: &IsoSurface, path: &Path) -> Option<Value> {
        iso.read(path)
            .await
            .unwrap()
            .map(|r| r.as_value().unwrap().clone())
    }

    #[tokio::test]
    async fn identity_paths() {
        let (cell, iso) = surface();
        assert_eq!(
            read_value(&iso, &path!("self/id")).await,
            Some(Value::from(cell.id.as_str()))
        );
        assert_eq!(
            read_value(&iso, &path!("self/state")).await,
            Some(Value::from("created"))
        );
    }

    #[tokio::test]
    async fn env_and_args() {
        let (_cell, iso) = surface_with(
            Arc::new(NullStdio),
            BTreeMap::from([("HOME".to_string(), "/blocks".to_string())]),
            vec!["prog".to_string(), "--flag".to_string()],
        );
        assert_eq!(
            read_value(&iso, &path!("env/HOME")).await,
            Some(Value::from("/blocks"))
        );
        assert_eq!(read_value(&iso, &path!("env/MISSING")).await, None);
        assert!(matches!(
            read_value(&iso, &path!("env")).await,
            Some(Value::Map(m)) if m.len() == 1
        ));
        assert_eq!(
            read_value(&iso, &path!("self/args")).await,
            Some(Value::Array(vec![
                Value::from("prog"),
                Value::from("--flag")
            ]))
        );
    }

    #[tokio::test]
    async fn stdio_round_trip() {
        let scripted = ScriptedStdio::with_input(["first line", "second"]);
        let (_cell, iso) = surface_with(Arc::new(scripted.clone()), BTreeMap::new(), Vec::new());

        assert_eq!(
            read_value(&iso, &path!("stdio/stdin")).await,
            Some(Value::from("first line"))
        );
        iso.write(&path!("stdio/stdout"), Value::from("out!"))
            .await
            .unwrap();
        iso.write(&path!("stdio/stderr"), Value::from("err!"))
            .await
            .unwrap();
        assert_eq!(scripted.output(), "out!err!");

        assert_eq!(
            read_value(&iso, &path!("stdio/stdin")).await,
            Some(Value::from("second"))
        );
        // EOF
        assert_eq!(read_value(&iso, &path!("stdio/stdin")).await, None);
    }

    #[tokio::test]
    async fn time_paths() {
        let (_cell, iso) = surface();
        let now = read_value(&iso, &path!("time/now")).await.unwrap();
        assert!(matches!(now, Value::String(s) if s.contains('T')));

        let ns = read_value(&iso, &path!("time/now_unix_ns")).await.unwrap();
        assert!(matches!(ns, Value::Integer(n) if n > 1_600_000_000_000_000_000));

        // A short sleep resolves.
        let slept = read_value(&iso, &path!("time/after/5")).await.unwrap();
        assert_eq!(slept, Value::Integer(5));
    }

    #[tokio::test]
    async fn sleep_is_cancelled_by_shutdown() {
        let (cell, iso) = surface();
        let iso = Arc::new(iso);
        let sleeper = {
            let iso = iso.clone();
            tokio::spawn(async move { iso.read(&path!("time/after/60000")).await })
        };
        tokio::task::yield_now().await;
        cell.cancel.cancel();
        let err = sleeper.await.unwrap().unwrap_err();
        assert!(err.is_cancelled());
    }

    #[tokio::test]
    async fn timers_deliver_to_mailbox() {
        let (cell, iso) = surface();
        let timer_path = iso
            .write(
                &path!("timers"),
                Value::Map(BTreeMap::from([
                    ("ms".to_string(), Value::Integer(5)),
                    ("tag".to_string(), Value::from("flush")),
                ])),
            )
            .await
            .unwrap();
        assert_eq!(&timer_path[0], "timers");

        let event = cell.next_event().await.unwrap().unwrap();
        assert!(matches!(
            event,
            BlockEvent::Timer { ref tag } if *tag == Value::from("flush")
        ));
    }

    #[tokio::test]
    async fn cancelled_timer_never_fires() {
        let (cell, iso) = surface();
        let timer_path = iso
            .write(
                &path!("timers"),
                Value::Map(BTreeMap::from([("ms".to_string(), Value::Integer(50_000))])),
            )
            .await
            .unwrap();
        iso.write(&timer_path, Value::Null).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(cell.pending_events().is_empty());
    }

    #[tokio::test]
    async fn exit_code_via_shutdown_complete() {
        let (cell, iso) = surface();
        iso.write(
            &path!("shutdown/complete"),
            Value::Map(BTreeMap::from([("code".to_string(), Value::Integer(7))])),
        )
        .await
        .unwrap();
        assert!(cell.shutdown_complete());
        assert_eq!(cell.exit_code(), 7);
    }

    #[tokio::test]
    async fn server_request_flow_through_iso() {
        let (cell, iso) = surface();
        let rx = cell.enqueue("read", path!("users/9"), Value::Null);

        let request = read_value(&iso, &path!("server/requests")).await.unwrap();
        let envelope = match crate::protocol::EventEnvelope::from_value(&request).unwrap() {
            crate::protocol::EventEnvelope::Request(envelope) => envelope,
            other => panic!("expected request, got {other:?}"),
        };
        assert_eq!(envelope.path, path!("users/9"));

        let respond_rel = envelope.respond_to.strip_prefix(&path!("iso")).unwrap();
        iso.write(&respond_rel, crate::protocol::ok_value(Value::from(1i64)))
            .await
            .unwrap();
        assert_eq!(
            crate::protocol::decode_read_response(rx.await.unwrap()).unwrap(),
            Some(Value::Integer(1))
        );
    }

    #[tokio::test]
    async fn proc_absent_when_not_granted() {
        let (_cell, iso) = surface();
        assert_eq!(read_value(&iso, &path!("proc")).await, None);
        let err = iso.write(&path!("proc"), Value::map()).await.unwrap_err();
        assert!(matches!(err, Error::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn unwritable_paths_are_permission_denied() {
        let (_cell, iso) = surface();
        let err = iso
            .write(&path!("time/now"), Value::from("x"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn shutdown_paths() {
        let (cell, iso) = surface();
        assert_eq!(
            read_value(&iso, &path!("shutdown/requested")).await,
            Some(Value::Bool(false))
        );
        cell.request_shutdown(crate::block::ShutdownMode::Graceful);
        assert_eq!(
            read_value(&iso, &path!("shutdown/requested")).await,
            Some(Value::Bool(true))
        );
        assert_eq!(
            read_value(&iso, &path!("shutdown/mode")).await,
            Some(Value::from("graceful"))
        );
        iso.write(&path!("shutdown/complete"), Value::map())
            .await
            .unwrap();
        assert!(cell.shutdown_complete());
        assert_eq!(cell.exit_code(), 0);
    }
}
