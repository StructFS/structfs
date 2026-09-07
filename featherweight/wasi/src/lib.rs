//! # featherweight-wasi
//!
//! The WASI-over-Isotope shim core
//! ([spec 10](https://github.com/StructFS/structfs/blob/main/isotope/spec/10-wasi-tower.md)).
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

use std::collections::BTreeMap;

use structfs_core_store::{path, Error, Path, PathComponent, Reader, Record, Value, Writer};

pub mod errno;
pub mod files;

pub use errno::Errno;
pub use files::MemFiles;

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

/// WASI whence values (preview1).
pub const SEEK_SET: u8 = 0;
pub const SEEK_CUR: u8 = 1;
pub const SEEK_END: u8 = 2;

/// Open flags for [`WasiIso::path_open`] (the preview1 subset).
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenFlags {
    pub read: bool,
    pub write: bool,
    /// Create the file if absent.
    pub create: bool,
    /// Truncate on open (expressed as the store's Null-delete).
    pub truncate: bool,
    /// All writes go to the end.
    pub append: bool,
}

/// One file-descriptor table entry.
enum FdEntry {
    /// A preopened directory: the guest-visible name and the namespace
    /// path it maps to (a preopen IS a mount — spec 10).
    Dir { guest: String, base: Path },
    /// An open file over the byte-stream pattern.
    File {
        path: Path,
        offset: u64,
        readable: bool,
        writable: bool,
        append: bool,
    },
}

/// Encode a guest-supplied relative POSIX path into namespace
/// components. POSIX names (`notes.txt`, `my-file`) are not valid path
/// components, so each segment crosses through Namecode
/// (`PathComponent::encode`) — lossless, deterministic, reversible.
/// `.` segments are skipped; `..` is rejected (no traversal above a
/// preopen).
fn encode_rel_path(base: &Path, rel: &str) -> Result<Path, Errno> {
    let mut path = base.clone();
    for segment in rel.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return Err(errno::NOTCAPABLE),
            name => path.push(PathComponent::encode(name)),
        }
    }
    Ok(path)
}

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
    /// Open files and preopened directories (fds 3 and up; 0-2 are the
    /// standard streams).
    fds: BTreeMap<u32, FdEntry>,
    next_fd: u32,
}

impl<S: Reader + Writer> WasiIso<S> {
    /// Build the shim over a block's namespace (or any store serving
    /// the `/iso/` shape), with no preopens.
    pub fn new(store: S) -> Self {
        Self::with_preopens(store, Vec::new())
    }

    /// Build the shim with preopened directories: each `(guest, base)`
    /// pair maps a guest-visible directory name (e.g. `"/data"`) onto a
    /// namespace path (typically a wired mount). Preopens receive fds
    /// starting at 3, in order — the layout libc discovers via
    /// `fd_prestat_*`.
    pub fn with_preopens(store: S, preopens: Vec<(String, Path)>) -> Self {
        let mut fds = BTreeMap::new();
        let mut next_fd = 3;
        for (guest, base) in preopens {
            fds.insert(next_fd, FdEntry::Dir { guest, base });
            next_fd += 1;
        }
        Self {
            store,
            stdin_buffer: Vec::new(),
            stdin_eof: false,
            fds,
            next_fd,
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

    /// `fd_write`: the standard streams (fd 1 and 2) and open files.
    ///
    /// Returns the number of bytes accepted.
    pub fn fd_write(&mut self, fd: u32, bytes: &[u8]) -> Result<usize, Errno> {
        match fd {
            1 | 2 => {
                let path = if fd == 1 {
                    path!("iso/stdio/stdout")
                } else {
                    path!("iso/stdio/stderr")
                };
                let text = String::from_utf8_lossy(bytes).into_owned();
                self.store
                    .write(&path, Record::parsed(Value::String(text)))
                    .map_err(|e| errno_from_error(&e))?;
                Ok(bytes.len())
            }
            0 => Err(errno::BADF),
            _ => self.file_write(fd, bytes),
        }
    }

    /// `fd_read`: stdin (fd 0) with `read(2)` semantics, and open files
    /// via ranged byte-stream reads.
    ///
    /// The `/iso/stdio/stdin` surface is line-oriented; the shim buffers
    /// a line (restoring its newline) and serves bytes from it.
    pub fn fd_read(&mut self, fd: u32, max: usize) -> Result<Vec<u8>, Errno> {
        if fd != 0 {
            return self.file_read(fd, max);
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

    // === files (the fd table, over the byte-stream pattern) ===

    /// The fds of the preopened directories, ascending.
    pub fn preopen_fds(&self) -> Vec<u32> {
        self.fds
            .iter()
            .filter(|(_, e)| matches!(e, FdEntry::Dir { .. }))
            .map(|(fd, _)| *fd)
            .collect()
    }

    /// `fd_prestat_dir_name`: the guest-visible name of a preopen.
    pub fn fd_prestat_dir_name(&self, fd: u32) -> Result<String, Errno> {
        match self.fds.get(&fd) {
            Some(FdEntry::Dir { guest, .. }) => Ok(guest.clone()),
            _ => Err(errno::BADF),
        }
    }

    fn file_len(&mut self, path: &Path) -> Result<Option<u64>, Errno> {
        match self.read_value(&path.child(PathComponent::try_new("len").unwrap()))? {
            Some(Value::Integer(len)) if len >= 0 => Ok(Some(len as u64)),
            Some(_) => Err(errno::IO),
            None => Ok(None),
        }
    }

    /// `path_open`: open `rel` (a POSIX-style relative path) under a
    /// preopened directory. Segments cross through Namecode encoding —
    /// POSIX names like `notes.txt` are not valid path components, and
    /// this is exactly the lossless embedding `PathComponent::encode`
    /// exists for. `..` never escapes the preopen (`ENOTCAPABLE`).
    pub fn path_open(&mut self, dirfd: u32, rel: &str, flags: OpenFlags) -> Result<u32, Errno> {
        let base = match self.fds.get(&dirfd) {
            Some(FdEntry::Dir { base, .. }) => base.clone(),
            _ => Err(errno::BADF)?,
        };
        let path = encode_rel_path(&base, rel)?;

        if flags.truncate {
            // O_TRUNC is the store's Null-delete.
            self.store
                .write(&path, Record::parsed(Value::Null))
                .map_err(|e| errno_from_error(&e))?;
        }
        let exists = self.file_len(&path)?.is_some();
        if !exists {
            if flags.create || flags.truncate {
                self.store
                    .write(&path, Record::parsed(Value::Bytes(Vec::new())))
                    .map_err(|e| errno_from_error(&e))?;
            } else {
                return Err(errno::NOENT);
            }
        }

        let offset = if flags.append {
            self.file_len(&path)?.unwrap_or(0)
        } else {
            0
        };
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(
            fd,
            FdEntry::File {
                path,
                offset,
                readable: flags.read || !(flags.write || flags.append),
                writable: flags.write || flags.append,
                append: flags.append,
            },
        );
        Ok(fd)
    }

    fn file_read(&mut self, fd: u32, max: usize) -> Result<Vec<u8>, Errno> {
        let (path, offset) = match self.fds.get(&fd) {
            Some(FdEntry::File {
                path,
                offset,
                readable: true,
                ..
            }) => (path.clone(), *offset),
            Some(FdEntry::File { .. }) => return Err(errno::ACCES),
            _ => return Err(errno::BADF),
        };
        let range = path
            .child(PathComponent::try_new("at").unwrap())
            .child(PathComponent::from(offset))
            .child(PathComponent::try_new("len").unwrap())
            .child(PathComponent::from(max as u64));
        let bytes = match self.read_value(&range)? {
            Some(Value::Bytes(bytes)) => bytes,
            Some(_) => return Err(errno::IO),
            None => return Err(errno::BADF), // file vanished under the fd
        };
        if let Some(FdEntry::File { offset, .. }) = self.fds.get_mut(&fd) {
            *offset += bytes.len() as u64;
        }
        Ok(bytes)
    }

    fn file_write(&mut self, fd: u32, bytes: &[u8]) -> Result<usize, Errno> {
        let (path, offset, append) = match self.fds.get(&fd) {
            Some(FdEntry::File {
                path,
                offset,
                writable: true,
                append,
                ..
            }) => (path.clone(), *offset, *append),
            Some(FdEntry::File { .. }) => return Err(errno::ACCES),
            _ => return Err(errno::BADF),
        };
        let target = if append {
            path.child(PathComponent::try_new("append").unwrap())
        } else {
            path.child(PathComponent::try_new("at").unwrap())
                .child(PathComponent::from(offset))
        };
        self.store
            .write(&target, Record::parsed(Value::Bytes(bytes.to_vec())))
            .map_err(|e| errno_from_error(&e))?;
        let new_offset = if append {
            self.file_len(&path)?.unwrap_or(offset + bytes.len() as u64)
        } else {
            offset + bytes.len() as u64
        };
        if let Some(FdEntry::File { offset, .. }) = self.fds.get_mut(&fd) {
            *offset = new_offset;
        }
        Ok(bytes.len())
    }

    /// `fd_seek`: returns the new offset.
    pub fn fd_seek(&mut self, fd: u32, delta: i64, whence: u8) -> Result<u64, Errno> {
        let (path, current) = match self.fds.get(&fd) {
            Some(FdEntry::File { path, offset, .. }) => (path.clone(), *offset),
            _ => return Err(errno::BADF),
        };
        let base = match whence {
            SEEK_SET => 0,
            SEEK_CUR => current as i64,
            SEEK_END => self.file_len(&path)?.ok_or(errno::BADF)? as i64,
            _ => return Err(errno::INVAL),
        };
        let target = base.checked_add(delta).ok_or(errno::INVAL)?;
        if target < 0 {
            return Err(errno::INVAL);
        }
        if let Some(FdEntry::File { offset, .. }) = self.fds.get_mut(&fd) {
            *offset = target as u64;
        }
        Ok(target as u64)
    }

    /// `fd_filestat`: the file's size in bytes.
    pub fn fd_filestat_size(&mut self, fd: u32) -> Result<u64, Errno> {
        let path = match self.fds.get(&fd) {
            Some(FdEntry::File { path, .. }) => path.clone(),
            _ => return Err(errno::BADF),
        };
        self.file_len(&path)?.ok_or(errno::BADF)
    }

    /// `fd_close`. Preopens cannot be closed (`ENOTSUP`), matching how
    /// libc treats them.
    pub fn fd_close(&mut self, fd: u32) -> Result<(), Errno> {
        match self.fds.get(&fd) {
            Some(FdEntry::File { .. }) => {
                self.fds.remove(&fd);
                Ok(())
            }
            Some(FdEntry::Dir { .. }) => Err(errno::NOTSUP),
            None => Err(errno::BADF),
        }
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

    /// iso/* -> FakeIso; files/* -> MemFiles — a two-mount namespace.
    struct TestNs {
        iso: FakeIso,
        files: MemFiles,
    }

    impl Reader for TestNs {
        fn read(&mut self, from: &Path) -> Result<Option<Record>, Error> {
            if !from.is_empty() && from[0] == "files" {
                self.files.read(&from.slice(1, from.len()))
            } else {
                self.iso.read(from)
            }
        }
    }

    impl Writer for TestNs {
        fn write(&mut self, to: &Path, data: Record) -> Result<Path, Error> {
            if !to.is_empty() && to[0] == "files" {
                self.files.write(&to.slice(1, to.len()), data)
            } else {
                self.iso.write(to, data)
            }
        }
    }

    fn fs_shim() -> WasiIso<TestNs> {
        WasiIso::with_preopens(
            TestNs {
                iso: FakeIso::default(),
                files: MemFiles::new(),
            },
            vec![("/data".to_string(), path!("files"))],
        )
    }

    #[test]
    fn preopens_are_discoverable() {
        let wasi = fs_shim();
        let preopens = wasi.preopen_fds();
        assert_eq!(preopens, vec![3]);
        assert_eq!(wasi.fd_prestat_dir_name(3).unwrap(), "/data");
        assert_eq!(wasi.fd_prestat_dir_name(9).unwrap_err(), errno::BADF);
    }

    #[test]
    fn file_round_trip_with_posix_names() {
        let mut wasi = fs_shim();

        // "notes.txt" is not a valid path component — Namecode carries it.
        let fd = wasi
            .path_open(
                3,
                "notes.txt",
                OpenFlags {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(wasi.fd_write(fd, b"hello ").unwrap(), 6);
        assert_eq!(wasi.fd_write(fd, b"files").unwrap(), 5);
        wasi.fd_close(fd).unwrap();

        let fd = wasi
            .path_open(
                3,
                "./notes.txt",
                OpenFlags {
                    read: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(wasi.fd_filestat_size(fd).unwrap(), 11);
        let mut contents = Vec::new();
        loop {
            let chunk = wasi.fd_read(fd, 4).unwrap();
            if chunk.is_empty() {
                break; // read(2): empty exactly at EOF
            }
            contents.extend_from_slice(&chunk);
        }
        assert_eq!(contents, b"hello files");
        wasi.fd_close(fd).unwrap();
        assert_eq!(wasi.fd_close(fd).unwrap_err(), errno::BADF);

        // The stored name is the Namecode encoding of "notes.txt".
        let encoded = structfs_core_store::PathComponent::encode("notes.txt");
        assert_eq!(encoded.decode().unwrap(), "notes.txt");
        assert!(wasi.store.files.get(&path!("").child(encoded)).is_some());
    }

    #[test]
    fn seek_all_whences() {
        let mut wasi = fs_shim();
        let fd = wasi
            .path_open(
                3,
                "seek.bin",
                OpenFlags {
                    read: true,
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        wasi.fd_write(fd, b"0123456789").unwrap();

        assert_eq!(wasi.fd_seek(fd, 2, SEEK_SET).unwrap(), 2);
        assert_eq!(wasi.fd_read(fd, 3).unwrap(), b"234");
        assert_eq!(wasi.fd_seek(fd, -1, SEEK_CUR).unwrap(), 4);
        assert_eq!(wasi.fd_seek(fd, -2, SEEK_END).unwrap(), 8);
        assert_eq!(wasi.fd_read(fd, 10).unwrap(), b"89");
        assert_eq!(wasi.fd_seek(fd, -99, SEEK_SET).unwrap_err(), errno::INVAL);
    }

    #[test]
    fn truncate_append_and_missing() {
        let mut wasi = fs_shim();

        // Missing without create: ENOENT.
        assert_eq!(
            wasi.path_open(
                3,
                "ghost",
                OpenFlags {
                    read: true,
                    ..Default::default()
                }
            )
            .unwrap_err(),
            errno::NOENT
        );

        // Escaping the preopen is a capability violation.
        assert_eq!(
            wasi.path_open(
                3,
                "../secrets",
                OpenFlags {
                    read: true,
                    ..Default::default()
                }
            )
            .unwrap_err(),
            errno::NOTCAPABLE
        );

        // Seed, truncate on open, then append twice.
        let fd = wasi
            .path_open(
                3,
                "log",
                OpenFlags {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        wasi.fd_write(fd, b"old contents").unwrap();
        wasi.fd_close(fd).unwrap();

        let fd = wasi
            .path_open(
                3,
                "log",
                OpenFlags {
                    write: true,
                    truncate: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(wasi.fd_filestat_size(fd).unwrap(), 0);
        wasi.fd_close(fd).unwrap();

        let fd = wasi
            .path_open(
                3,
                "log",
                OpenFlags {
                    append: true,
                    ..Default::default()
                },
            )
            .unwrap();
        wasi.fd_write(fd, b"a").unwrap();
        wasi.fd_write(fd, b"b").unwrap();
        assert_eq!(wasi.fd_filestat_size(fd).unwrap(), 2);

        // Reading a write-only fd is EACCES.
        assert_eq!(wasi.fd_read(fd, 1).unwrap_err(), errno::ACCES);
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
