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
use featherweight_wasi::{errno, WasiIso, CLOCK_MONOTONIC, CLOCK_REALTIME};
use structfs_core_store::Error;

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
        let mut wasi = WasiIso::new(&mut *ns);
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

    let mut runtime = Runtime::new().with_stdio_provider(Arc::new(move |name| {
        (name == "posix").then(|| Arc::new(provided.clone()) as Arc<dyn Stdio>)
    }));
    register_builtins(&mut runtime);
    runtime.register_builtin(
        "posix",
        Arc::new(|| Box::new(PosixBlock) as Box<dyn NativeBlock>),
    );

    let def = AssemblyDef::from_str(
        r#"{"assembly": "wasi-tower",
            "blocks": {"posix": {"artifact": "builtin:posix",
                                 "args": ["prog", "--demo"],
                                 "env": {"HOME": "/blocks"}}},
            "public": "posix"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), std::env::temp_dir().as_path())
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
        output.contains("cat: hello tower\n"),
        "stdin echo missing: {output}"
    );
    assert!(
        output.contains("cat: second line\n"),
        "stdin echo missing: {output}"
    );

    assembly.shutdown(Duration::from_secs(2)).await;
}
