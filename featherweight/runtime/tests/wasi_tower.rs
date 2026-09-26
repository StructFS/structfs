//! The WASI tower, end to end (`isotope/spec/10-wasi-tower.md`):
//! a POSIX-style program runs through the `featherweight-wasi` shim
//! over a block's REAL namespace — every "syscall" is store traffic on
//! the `/iso/` surface, and the runtime knows nothing about WASI.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use featherweight_runtime::{
    register_builtins, AssemblyDef, BlockState, NativeBlock, Runtime, ScriptedStdio, Stdio,
};
use featherweight_wasi::{
    errno, MemFiles, OpenFlags, WasiIso, CLOCK_MONOTONIC, CLOCK_REALTIME, SEEK_SET,
};
use structfs_core_store::{path, Error};

/// A "POSIX program": knows nothing about StructFS — only the WASI-ish
/// surface the shim provides.
fn posix_main(
    wasi: &mut WasiIso<&mut featherweight_runtime::Namespace>,
) -> Result<u32, errno::Errno> {
    // argv / environ
    let args = wasi.args()?;
    let environ = wasi.environ()?;
    wasi.fd_write(1, format!("args: {}\n", args.join(" ")).as_bytes())?;
    for (name, value) in &environ {
        wasi.fd_write(1, format!("env: {name}={value}\n").as_bytes())?;
    }

    // clocks + randomness
    let t1 = wasi.clock_time_get(CLOCK_MONOTONIC)?;
    wasi.poll_oneoff_sleep(2_000_000)?; // 2ms
    let t2 = wasi.clock_time_get(CLOCK_MONOTONIC)?;
    if t2 <= t1 {
        wasi.fd_write(2, b"clock went backwards!\n")?;
        return Ok(1);
    }
    if wasi.clock_time_get(CLOCK_REALTIME)? < 1_600_000_000_000_000_000 {
        return Ok(1);
    }
    let noise = wasi.random_get(8)?;
    wasi.fd_write(1, format!("random: {} bytes\n", noise.len()).as_bytes())?;

    // files: discover the preopen, then a create/write/reopen/read
    // round trip — every byte moves as byte-stream store traffic.
    let dirfd = *wasi.preopen_fds().first().ok_or(errno::NOENT)?;
    let dir = wasi.fd_prestat_dir_name(dirfd)?;
    let fd = wasi.path_open(
        dirfd,
        "out.txt",
        OpenFlags::default().with_write(true).with_create(true),
    )?;
    wasi.fd_write(fd, b"file contents")?;
    wasi.fd_close(fd)?;
    let fd = wasi.path_open(dirfd, "out.txt", OpenFlags::default().with_read(true))?;
    let size = wasi.fd_filestat_size(fd)?;
    wasi.fd_seek(fd, 5, SEEK_SET)?;
    let tail = wasi.fd_read(fd, 64)?;
    wasi.fd_write(
        1,
        format!(
            "file {dir}/out.txt [{size}]: {}\n",
            String::from_utf8_lossy(&tail)
        )
        .as_bytes(),
    )?;
    wasi.fd_close(fd)?;

    // cat(1): stdin -> stdout until EOF (read(2) returns 0)
    loop {
        let bytes = wasi.fd_read(0, 64)?;
        if bytes.is_empty() {
            break;
        }
        wasi.fd_write(1, b"cat: ")?;
        wasi.fd_write(1, &bytes)?;
    }

    Ok(3)
}

struct PosixBlock;

impl NativeBlock for PosixBlock {
    fn run(&mut self, ns: &mut featherweight_runtime::Namespace) -> Result<(), Error> {
        let mut wasi =
            WasiIso::with_preopens(&mut *ns, vec![("/data".to_string(), path!("files"))]);
        let code = posix_main(&mut wasi).unwrap_or(70);
        wasi.proc_exit(code)
            .map_err(|e| Error::store("posix", "exit", format!("errno {e}")))?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn posix_program_runs_on_the_iso_surface() {
    let stdio = ScriptedStdio::with_input(["hello tower", "second line"]);
    let provided = stdio.clone();

    let mut config = featherweight_runtime::RuntimeConfig::new(tokio::runtime::Handle::current())
        .with_stdio_provider(Arc::new(move |name| {
            (name == "posix").then(|| Arc::new(provided.clone()) as Arc<dyn Stdio>)
        }));
    register_builtins(&mut config);
    config.register_builtin(
        "posix",
        Arc::new(|| Box::new(PosixBlock) as Box<dyn NativeBlock>),
    );
    let runtime = Runtime::new(config);

    let def = AssemblyDef::from_str(
        r#"{"assembly": "wasi-tower",
            "blocks": {"posix": {"artifact": "builtin:posix",
                                 "args": ["prog", "--demo"],
                                 "env": {"HOME": "/blocks"}}},
            "public": "posix",
            "imports": {"data": "The preopened file tree"},
            "wiring": ["posix:/files -> $data"]}"#,
    )
    .unwrap();
    let mut imports = HashMap::new();
    imports.insert(
        "data".to_string(),
        featherweight_runtime::host_store(MemFiles::new()),
    );
    let assembly = runtime
        .instantiate(&def, imports, std::env::temp_dir().as_path())
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), assembly.wait_public_terminal())
        .await
        .expect("posix block did not finish");

    // Exit code declared via proc_exit -> shutdown/complete.
    assert_eq!(assembly.public_cell().state(), BlockState::Stopped);
    assert_eq!(assembly.public_cell().exit_code(), 3);

    let output = stdio.output();
    assert!(
        output.contains("args: prog --demo"),
        "argv missing: {output}"
    );
    assert!(
        output.contains("env: HOME=/blocks"),
        "environ missing: {output}"
    );
    assert!(
        output.contains("random: 8 bytes"),
        "random missing: {output}"
    );
    assert!(
        output.contains("file /data/out.txt [13]: contents\n"),
        "file round trip missing: {output}"
    );
    assert!(
        output.contains("cat: hello tower\n"),
        "stdin echo missing: {output}"
    );
    assert!(
        output.contains("cat: second line\n"),
        "stdin echo missing: {output}"
    );

    assembly.shutdown(Duration::from_secs(2)).await;
}

/// The shim's errno table (spec 09's errno column) agrees with the
/// runtime's error taxonomy for every core error variant. The shim cannot
/// depend on the runtime, so this is where the two tables are held together.
#[test]
fn wasi_errno_agrees_with_the_error_taxonomy() {
    use featherweight_runtime::ErrorKind;
    use structfs_core_store::{CodecErrorKind, CodecOperation, Format, PathError};
    let samples = [
        Error::Path(PathError::InvalidComponent {
            component: "a-b".into(),
            position: 0,
            message: "not an identifier".into(),
        }),
        Error::NoRoute { path: path!("x") },
        Error::Codec {
            kind: CodecErrorKind::Syntax,
            operation: CodecOperation::Decode,
            format: Format::JSON,
            message: "bad".into(),
        },
        Error::Codec {
            kind: CodecErrorKind::ResourceLimit,
            operation: CodecOperation::Decode,
            format: Format::JSON,
            message: "deep".into(),
        },
        Error::UnsupportedFormat(Format::OCTET_STREAM),
        Error::Ll(structfs_ll_store::LLError::NotSupported),
        Error::Io(std::io::Error::other("disk")),
        Error::store("s", "op", "weird"),
        Error::not_found(path!("x")),
        Error::permission_denied("no"),
        Error::conflict("dup"),
        Error::invalid_argument("zero"),
        Error::overloaded("busy"),
        Error::deadline_exceeded("late"),
        Error::resource_limit("big"),
        Error::cancelled("gone"),
    ];
    let mut kinds = std::collections::HashSet::new();
    for error in samples {
        let kind = ErrorKind::of(&error);
        kinds.insert(kind);
        let expected = match kind {
            ErrorKind::NotFound | ErrorKind::NoRoute => errno::NOENT,
            ErrorKind::PermissionDenied => errno::NOTCAPABLE,
            ErrorKind::Conflict => errno::EXIST,
            ErrorKind::Overloaded => errno::AGAIN,
            ErrorKind::DeadlineExceeded => errno::TIMEDOUT,
            ErrorKind::Cancelled => errno::INTR,
            ErrorKind::InvalidPath | ErrorKind::InvalidArgument => errno::INVAL,
            // Resource limits have no errno analogue (spec 11); they and
            // every untyped failure surface as EIO.
            _ => errno::IO,
        };
        assert_eq!(
            featherweight_wasi::errno_from_error(&error),
            expected,
            "{error}"
        );
    }
    assert_eq!(kinds.len(), ErrorKind::ALL.len());
}
