//! The `/iso/` system store (`isotope/spec/04-system-paths.md`).
//!
//! Every block's namespace mounts this surface at `iso/`. It is Isotope's
//! syscall interface: identity, lifecycle, time, randomness, logging, and
//! the server protocol — all served as reads and writes.

use std::collections::BTreeMap;
use std::sync::Arc;

use structfs_core_store::{Error, Path, Record, Value};

use crate::block::{BlockCell, ServerRequest};

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

/// The per-block `/iso/` surface. Paths are relative to the `iso` mount.
pub struct IsoSurface {
    cell: Arc<BlockCell>,
    log: Arc<dyn LogSink>,
}

const SECTIONS: [(&str, &str); 6] = [
    ("server", "Server protocol: requests, responses"),
    ("self", "Block identity: id, state, interface"),
    ("shutdown", "Lifecycle control: requested, mode, complete"),
    ("time", "Time services: now, monotonic, zone"),
    ("random", "Randomness: uuid, int, bytes/{n}"),
    ("log", "Logging: debug, info, warn, error"),
];

impl IsoSurface {
    /// Create the surface for a block.
    pub fn new(cell: Arc<BlockCell>, log: Arc<dyn LogSink>) -> Self {
        Self { cell, log }
    }

    fn directory() -> Value {
        Value::Map(
            SECTIONS
                .iter()
                .map(|(name, doc)| (name.to_string(), Value::from(*doc)))
                .collect(),
        )
    }

    /// Serve a read. May park (server/requests), so this is async; the
    /// namespace bridges it onto the block's thread.
    pub async fn read(&self, path: &Path) -> Result<Option<Record>, Error> {
        if path.is_empty() {
            return Ok(Some(Record::parsed(Self::directory())));
        }
        let value = match (path.len(), path[0].as_str()) {
            // === server ===
            (1, "server") => Some(Value::Map(BTreeMap::from([
                (
                    "requests".to_string(),
                    Value::from("read: next request (blocking)"),
                ),
                (
                    "responses".to_string(),
                    Value::from("write responses/{token}"),
                ),
            ]))),
            (2, "server") if path[1] == "requests" => {
                return match self.cell.next_request().await? {
                    Some(request) => Ok(Some(Record::parsed(request.to_value()))),
                    // Shutdown: unblock with Null (spec 07).
                    None => Ok(Some(Record::parsed(Value::Null))),
                };
            }
            (3, "server") if path[1] == "requests" && path[2] == "pending" => Some(Value::Array(
                self.cell
                    .pending_requests()
                    .iter()
                    .map(ServerRequest::to_value)
                    .collect(),
            )),
            // === self ===
            (1, "self") => Some(Value::Map(BTreeMap::from([
                ("id".to_string(), Value::from(self.cell.id.as_str())),
                ("state".to_string(), Value::from(self.cell.state().as_str())),
            ]))),
            (2, "self") if path[1] == "id" => Some(Value::from(self.cell.id.as_str())),
            (2, "self") if path[1] == "state" => Some(Value::from(self.cell.state().as_str())),
            (2, "self") if path[1] == "interface" => self.cell.interface(),
            (2, "self") if path[1] == "last_error" => self.cell.last_error().map(Value::from),
            // === shutdown ===
            (2, "shutdown") if path[1] == "requested" => {
                Some(Value::Bool(self.cell.shutdown_requested()))
            }
            (2, "shutdown") if path[1] == "mode" => self
                .cell
                .shutdown_mode()
                .map(|mode| Value::from(mode.as_str())),
            // === time ===
            (2, "time") if path[1] == "now" => {
                Some(Value::String(chrono::Utc::now().to_rfc3339()))
            }
            (2, "time") if path[1] == "monotonic" => {
                Some(Value::Integer(self.cell.monotonic_nanos()))
            }
            (2, "time") if path[1] == "zone" => Some(Value::from("UTC")),
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

    /// Serve a write. All writes are immediate.
    pub fn write(&self, path: &Path, value: Value) -> Result<Path, Error> {
        match (path.len(), path.iter().map(String::as_str).collect::<Vec<_>>().as_slice()) {
            (3, ["server", "responses", token]) => {
                let token: u64 = token
                    .parse()
                    .map_err(|_| Error::store("iso", "respond", "bad response token"))?;
                self.cell.respond(token, value);
                Ok(path.clone())
            }
            (2, ["self", "interface"]) => {
                self.cell.set_interface(value);
                Ok(path.clone())
            }
            (2, ["shutdown", "complete"]) => {
                self.cell.mark_shutdown_complete();
                Ok(path.clone())
            }
            (2, ["log", level @ ("debug" | "info" | "warn" | "error")]) => {
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
    use std::sync::Mutex;
    use structfs_core_store::path;

    fn surface() -> (Arc<BlockCell>, IsoSurface) {
        let cell = Arc::new(BlockCell::new("test", FailurePolicy::FailFast));
        let iso = IsoSurface::new(cell.clone(), Arc::new(StderrLog));
        (cell, iso)
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
    async fn time_and_random_paths() {
        let (_cell, iso) = surface();
        let now = read_value(&iso, &path!("time/now")).await.unwrap();
        assert!(matches!(now, Value::String(s) if s.contains('T')));

        let mono = read_value(&iso, &path!("time/monotonic")).await.unwrap();
        assert!(matches!(mono, Value::Integer(n) if n >= 0));

        let uuid = read_value(&iso, &path!("random/uuid")).await.unwrap();
        assert!(matches!(uuid, Value::String(s) if s.len() == 36));

        let bytes = read_value(&iso, &path!("random/bytes/10")).await.unwrap();
        assert!(matches!(bytes, Value::Bytes(b) if b.len() == 10));
    }

    #[tokio::test]
    async fn interface_round_trip() {
        let (_cell, iso) = surface();
        assert_eq!(read_value(&iso, &path!("self/interface")).await, None);
        iso.write(&path!("self/interface"), Value::from("declared"))
            .unwrap();
        assert_eq!(
            read_value(&iso, &path!("self/interface")).await,
            Some(Value::from("declared"))
        );
    }

    #[tokio::test]
    async fn server_request_flow_through_iso() {
        let (cell, iso) = surface();
        let rx = cell.enqueue("read", path!("users/9"), Value::Null);

        let request = read_value(&iso, &path!("server/requests")).await.unwrap();
        let envelope = crate::protocol::RequestEnvelope::from_value(&request).unwrap();
        assert_eq!(envelope.path, path!("users/9"));

        // Respond through the respond_to path (strip the iso/ mount).
        let respond_rel = envelope.respond_to.strip_prefix(&path!("iso")).unwrap();
        iso.write(&respond_rel, crate::protocol::ok_value(Value::from(1i64)))
            .unwrap();
        assert_eq!(
            crate::protocol::decode_read_response(rx.await.unwrap()).unwrap(),
            Some(Value::Integer(1))
        );
    }

    #[tokio::test]
    async fn log_writes_reach_sink() {
        struct Capture(Mutex<Vec<String>>);
        impl LogSink for Capture {
            fn log(&self, block: &str, level: &str, message: &Value) {
                self.0
                    .lock()
                    .unwrap()
                    .push(format!("{block}/{level}/{message:?}"));
            }
        }
        let cell = Arc::new(BlockCell::new("noisy", FailurePolicy::FailFast));
        let sink = Arc::new(Capture(Mutex::new(Vec::new())));
        let iso = IsoSurface::new(cell, sink.clone());

        iso.write(&path!("log/info"), Value::from("hello")).unwrap();
        assert_eq!(sink.0.lock().unwrap().len(), 1);

        // Unknown levels are not writable.
        assert!(iso.write(&path!("log/loud"), Value::from("x")).is_err());
    }

    #[tokio::test]
    async fn unwritable_paths_are_permission_denied() {
        let (_cell, iso) = surface();
        let err = iso.write(&path!("time/now"), Value::from("x")).unwrap_err();
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
        iso.write(&path!("shutdown/complete"), Value::map()).unwrap();
        assert!(cell.shutdown_complete());
    }
}
