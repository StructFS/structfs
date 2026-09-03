//! # featherweight-wasi
//!
//! The WASI-over-Isotope shim core (`isotope/spec/10-wasi-tower.md`).
//!
//! Isotope does not depend on WASI; WASI is a compatibility layer that
//! bottoms out in the Block ABI's two functions. This crate implements
//! the syscall surface **generically over any StructFS store** — the
//! block's namespace on a real runtime, a fake in tests — so the
//! mapping is developed and verified natively, without a wasm
//! toolchain. The wasm packaging (the same core behind
//! `wasi_snapshot_preview1` exports, composed onto stock binaries at
//! load time) is the thin outer layer, merged from the shim lineage.
//!
//! Scope today: the preview1 subset the `/iso/` surface serves — args,
//! environ, clocks, random, stdio, `proc_exit`, clock-only
//! `poll_oneoff` — plus the errno mapping for the whole typed error
//! taxonomy. Filesystem and socket fds arrive with wired byte-stream
//! mounts.

use structfs_core_store::{path, Error, Path, PathComponent, Reader, Record, Value, Writer};

pub mod errno;

pub use errno::Errno;

/// Map a typed store error onto a WASI errno (spec 10's table).
pub fn errno_from_error(error: &Error) -> Errno {
    match error {
        Error::NotFound { .. } | Error::NoRoute { .. } => errno::NOENT,
        Error::PermissionDenied { .. } => errno::NOTCAPABLE,
        Error::Overloaded { .. } => errno::AGAIN,
        Error::DeadlineExceeded { .. } => errno::TIMEDOUT,
        Error::Conflict { .. } => errno::EXIST,
        Error::Cancelled { .. } => errno::INTR,
        Error::Path(_) => errno::INVAL,
        _ => errno::IO,
    }
}

/// WASI clock ids (preview1).
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;

/// The shim: WASI syscalls over a block namespace.
///
/// `S` is anything that reads and writes the block's paths — on a real
/// runtime, the namespace handed to the block. All I/O the syscalls
/// perform is store traffic; there is no other channel.
pub struct WasiIso<S> {
    store: S,
    /// Line-oriented stdin refill buffer (`iso/stdio/stdin` serves
    /// lines; `fd_read` serves bytes).
    stdin_buffer: Vec<u8>,
    stdin_eof: bool,
}

impl<S: Reader + Writer> WasiIso<S> {
    /// Build the shim over a block's namespace (or any store serving
    /// the `/iso/` shape).
    pub fn new(store: S) -> Self {
        Self {
            store,
            stdin_buffer: Vec::new(),
            stdin_eof: false,
        }
    }

    /// Unwrap, returning the namespace.
    pub fn into_inner(self) -> S {
        self.store
    }

    fn read_value(&mut self, path: &Path) -> Result<Option<Value>, Errno> {
        match self.store.read(path) {
            Ok(record) => Ok(record.and_then(|r| r.as_value().cloned())),
            Err(e) => Err(errno_from_error(&e)),
        }
    }

    // === args / environ ===

    /// `args_get`: the block's argument vector.
    pub fn args(&mut self) -> Result<Vec<String>, Errno> {
        match self.read_value(&path!("iso/self/args"))? {
            Some(Value::Array(items)) => Ok(items
                .into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    /// `environ_get`: the block's environment, sorted by name.
    pub fn environ(&mut self) -> Result<Vec<(String, String)>, Errno> {
        match self.read_value(&path!("iso/env"))? {
            Some(Value::Map(map)) => Ok(map
                .into_iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k, s)),
                    _ => None,
                })
                .collect()),
            _ => Ok(Vec::new()),
        }
    }

    // === clocks ===

    /// `clock_time_get`: nanoseconds for the given clock.
    pub fn clock_time_get(&mut self, clock_id: u32) -> Result<u64, Errno> {
        let path = match clock_id {
            CLOCK_REALTIME => path!("iso/time/now_unix_ns"),
            CLOCK_MONOTONIC => path!("iso/time/monotonic"),
            _ => return Err(errno::INVAL),
        };
        match self.read_value(&path)? {
            Some(Value::Integer(ns)) if ns >= 0 => Ok(ns as u64),
            _ => Err(errno::IO),
        }
    }

    // === random ===

    /// `random_get`: `len` random bytes.
    pub fn random_get(&mut self, len: usize) -> Result<Vec<u8>, Errno> {
        let path = path!("iso/random/bytes").child(PathComponent::from(len as u64));
        match self.read_value(&path)? {
            Some(Value::Bytes(bytes)) => Ok(bytes),
            _ => Err(errno::IO),
        }
    }

    // === stdio ===

    /// `fd_write` for the standard streams (fd 1 and 2).
    ///
    /// Returns the number of bytes accepted.
    pub fn fd_write(&mut self, fd: u32, bytes: &[u8]) -> Result<usize, Errno> {
        let path = match fd {
            1 => path!("iso/stdio/stdout"),
            2 => path!("iso/stdio/stderr"),
            _ => return Err(errno::BADF),
        };
        let text = String::from_utf8_lossy(bytes).into_owned();
        self.store
            .write(&path, Record::parsed(Value::String(text)))
            .map_err(|e| errno_from_error(&e))?;
        Ok(bytes.len())
    }

    /// `fd_read` for stdin (fd 0), `read(2)` semantics: parks for input,
    /// returns 0 bytes exactly at EOF.
    ///
    /// The `/iso/stdio/stdin` surface is line-oriented; the shim buffers
    /// a line (restoring its newline) and serves bytes from it.
    pub fn fd_read(&mut self, fd: u32, max: usize) -> Result<Vec<u8>, Errno> {
        if fd != 0 {
            return Err(errno::BADF);
        }
        if self.stdin_buffer.is_empty() && !self.stdin_eof {
            match self.read_value(&path!("iso/stdio/stdin"))? {
                Some(Value::String(line)) => {
                    self.stdin_buffer.extend_from_slice(line.as_bytes());
                    self.stdin_buffer.push(b'\n');
                }
                _ => self.stdin_eof = true,
            }
        }
        let take = max.min(self.stdin_buffer.len());
        Ok(self.stdin_buffer.drain(..take).collect())
    }

    // === process ===

    /// `proc_exit`: record the exit code. The caller must then return
    /// from the block's run loop — the shim cannot unwind for it.
    pub fn proc_exit(&mut self, code: u32) -> Result<(), Errno> {
        let mut map = std::collections::BTreeMap::new();
        map.insert("code".to_string(), Value::Integer(code as i64));
        self.store
            .write(
                &path!("iso/shutdown/complete"),
                Record::parsed(Value::Map(map)),
            )
            .map_err(|e| errno_from_error(&e))?;
        Ok(())
    }

    // === poll ===

    /// `poll_oneoff` for a single relative clock subscription: sleep.
    ///
    /// General fd/multi-subscription polling maps onto the mailbox and
    /// is out of the strawman subset.
    pub fn poll_oneoff_sleep(&mut self, nanoseconds: u64) -> Result<(), Errno> {
        let ms = nanoseconds.div_ceil(1_000_000);
        let path = path!("iso/time/after").child(PathComponent::from(ms));
        self.read_value(&path)?;
        Ok(())
    }

    /// `sched_yield`: a no-op — blocks are cooperatively scheduled by
    /// their parked reads.
    pub fn sched_yield(&mut self) -> Result<(), Errno> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};

    /// A store serving just enough of the `/iso/` shape for the shim.
    #[derive(Default)]
    struct FakeIso {
        stdin: VecDeque<String>,
        stdout: String,
        stderr: String,
        slept_ms: Vec<u64>,
        exit_code: Option<i64>,
        clock: i64,
    }

    impl Reader for FakeIso {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            let components: Vec<&str> = from.iter().map(String::as_str).collect();
            let value = match components.as_slice() {
                ["iso", "self", "args"] => Some(Value::Array(vec![
                    Value::from("prog"),
                    Value::from("--flag"),
                ])),
                ["iso", "env"] => Some(Value::Map(BTreeMap::from([(
                    "HOME".to_string(),
                    Value::from("/blocks"),
                )]))),
                ["iso", "time", "now_unix_ns"] => {
                    self.clock += 1;
                    Some(Value::Integer(1_700_000_000_000_000_000 + self.clock))
                }
                ["iso", "time", "monotonic"] => {
                    self.clock += 1;
                    Some(Value::Integer(self.clock))
                }
                ["iso", "time", "after", ms] => {
                    self.slept_ms.push(ms.parse().unwrap());
                    Some(Value::Integer(0))
                }
                ["iso", "random", "bytes", n] => Some(Value::Bytes(vec![7u8; n.parse().unwrap()])),
                ["iso", "stdio", "stdin"] => self.stdin.pop_front().map(Value::String),
                _ => None,
            };
            Ok(value.map(Record::parsed))
        }
    }

    impl Writer for FakeIso {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            let value = data.as_value().cloned().unwrap_or(Value::Null);
            let components: Vec<&str> = to.iter().map(String::as_str).collect();
            match components.as_slice() {
                ["iso", "stdio", "stdout"] => {
                    if let Value::String(s) = value {
                        self.stdout.push_str(&s);
                    }
                }
                ["iso", "stdio", "stderr"] => {
                    if let Value::String(s) = value {
                        self.stderr.push_str(&s);
                    }
                }
                ["iso", "shutdown", "complete"] => {
                    if let Value::Map(map) = value {
                        if let Some(Value::Integer(code)) = map.get("code") {
                            self.exit_code = Some(*code);
                        }
                    }
                }
                _ => return Err(Error::permission_denied("not wired")),
            }
            Ok(to.clone())
        }
    }

    fn shim() -> WasiIso<FakeIso> {
        WasiIso::new(FakeIso::default())
    }

    #[test]
    fn args_and_environ() {
        let mut wasi = shim();
        assert_eq!(wasi.args().unwrap(), vec!["prog", "--flag"]);
        assert_eq!(
            wasi.environ().unwrap(),
            vec![("HOME".to_string(), "/blocks".to_string())]
        );
    }

    #[test]
    fn clocks() {
        let mut wasi = shim();
        let realtime = wasi.clock_time_get(CLOCK_REALTIME).unwrap();
        assert!(realtime > 1_600_000_000_000_000_000);
        let m1 = wasi.clock_time_get(CLOCK_MONOTONIC).unwrap();
        let m2 = wasi.clock_time_get(CLOCK_MONOTONIC).unwrap();
        assert!(m2 > m1);
        assert_eq!(wasi.clock_time_get(99).unwrap_err(), errno::INVAL);
    }

    #[test]
    fn random() {
        let mut wasi = shim();
        assert_eq!(wasi.random_get(16).unwrap().len(), 16);
    }

    #[test]
    fn stdout_and_stderr() {
        let mut wasi = shim();
        assert_eq!(wasi.fd_write(1, b"hello ").unwrap(), 6);
        wasi.fd_write(1, b"world\n").unwrap();
        wasi.fd_write(2, b"oops\n").unwrap();
        assert_eq!(wasi.fd_write(7, b"x").unwrap_err(), errno::BADF);
        let fake = wasi.into_inner();
        assert_eq!(fake.stdout, "hello world\n");
        assert_eq!(fake.stderr, "oops\n");
    }

    #[test]
    fn stdin_serves_bytes_over_lines_and_eofs() {
        let mut wasi = shim();
        wasi.store.stdin.push_back("first".to_string());
        wasi.store.stdin.push_back("second".to_string());

        // Partial reads drain the buffered line before refilling.
        assert_eq!(wasi.fd_read(0, 3).unwrap(), b"fir");
        assert_eq!(wasi.fd_read(0, 100).unwrap(), b"st\n");
        assert_eq!(wasi.fd_read(0, 100).unwrap(), b"second\n");
        // read(2): zero bytes exactly at EOF, stable thereafter.
        assert_eq!(wasi.fd_read(0, 100).unwrap(), b"");
        assert_eq!(wasi.fd_read(0, 100).unwrap(), b"");
        assert_eq!(wasi.fd_read(3, 1).unwrap_err(), errno::BADF);
    }

    #[test]
    fn proc_exit_records_code() {
        let mut wasi = shim();
        wasi.proc_exit(42).unwrap();
        assert_eq!(wasi.into_inner().exit_code, Some(42));
    }

    #[test]
    fn poll_sleep_rounds_up_to_ms() {
        let mut wasi = shim();
        wasi.poll_oneoff_sleep(1_500_000).unwrap(); // 1.5ms -> 2ms
        wasi.sched_yield().unwrap();
        assert_eq!(wasi.into_inner().slept_ms, vec![2]);
    }

    #[test]
    fn errno_mapping_is_typed() {
        assert_eq!(
            errno_from_error(&Error::not_found(path!("x"))),
            errno::NOENT
        );
        assert_eq!(
            errno_from_error(&Error::permission_denied("no")),
            errno::NOTCAPABLE
        );
        assert_eq!(errno_from_error(&Error::overloaded("busy")), errno::AGAIN);
        assert_eq!(
            errno_from_error(&Error::deadline_exceeded("late")),
            errno::TIMEDOUT
        );
        assert_eq!(errno_from_error(&Error::conflict("dup")), errno::EXIST);
        assert_eq!(errno_from_error(&Error::cancelled("gone")), errno::INTR);
        assert_eq!(
            errno_from_error(&Error::store("s", "op", "weird")),
            errno::IO
        );
    }

    #[test]
    fn store_denial_surfaces_as_notcapable() {
        let mut wasi = shim();
        // FakeIso denies writes outside its wired paths; the shim turns
        // that into ENOTCAPABLE — WASI's own capability errno.
        let err = wasi
            .store
            .write(&path!("elsewhere"), Record::parsed(Value::Null))
            .unwrap_err();
        assert_eq!(errno_from_error(&err), errno::NOTCAPABLE);
    }
}
