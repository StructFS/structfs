//! End-to-end checks of the shipped entry point, including file-loaded prepared
//! code and recording/replay. These exercise Runtime::with_handle outside block_on.
use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fw"))
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn fixture(dir: &Path) {
    std::fs::write(
        dir.join("guest.wasm"),
        r#"(module
      (import "structfs" "read" (func $read (param i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "{\"serialization\":\"application/json\"}")
      (data (i32.const 128) "iso/self/id")
      (func (export "block_alloc") (param i32) (result i32) i32.const 4096)
      (func (export "manifest") (param $p i32) (result i32)
        local.get $p i32.const 0 i32.store
        local.get $p i32.const 4 i32.add i32.const 36 i32.store i32.const 0)
      (func (export "run") (result i32)
        i32.const 128 i32.const 11 i32.const 1024 call $read))"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("assembly.json"),
        r#"{"assembly":"cli_test","blocks":{"main":"guest.wasm"},"public":"main"}"#,
    )
    .unwrap();
}
#[test]
fn cli_loads_prepared_code_and_records_replays_and_seeks() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    std::fs::create_dir(dir.path().join("recording")).unwrap();
    success(run(dir.path(), &["run", "assembly.json"]));
    success(run(
        dir.path(),
        &[
            "run",
            "assembly.json",
            "--seed",
            "7",
            "--record",
            "recording",
        ],
    ));
    assert!(dir.path().join("recording/session.jsonl").exists());
    success(run(
        dir.path(),
        &["run", "assembly.json", "--replay", "recording"],
    ));
    success(run(
        dir.path(),
        &["run", "assembly.json", "--seek", "recording"],
    ));
    success(run(
        dir.path(),
        &["run", "assembly.json", "--seek", "recording", "--at", "0"],
    ));
    success(run(
        dir.path(),
        &[
            "run",
            "assembly.json",
            "--sim",
            "3",
            "--session",
            "session.jsonl",
        ],
    ));
}
#[test]
fn cli_rejects_invalid_flags_and_files_with_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    for args in [
        vec![],
        vec!["run"],
        vec!["--record"],
        vec!["--replay"],
        vec!["--seek"],
        vec!["--session"],
        vec!["--seed"],
        vec!["--seed", "bad"],
        vec!["--sim"],
        vec!["--sim", "bad"],
        vec!["--record", "a", "--replay", "b"],
        vec!["--record", "a", "--seek", "b"],
        vec!["--seed", "1", "--sim", "2"],
        vec!["--seek", "missing", "--at", "bad"],
        vec!["--seek", "missing", "--at", "1"],
    ] {
        assert_eq!(run(dir.path(), &args).status.code(), Some(2), "{args:?}");
    }
    std::fs::write(dir.path().join("invalid.json"), "not an assembly").unwrap();
    std::fs::create_dir(dir.path().join("bad_recording")).unwrap();
    std::fs::write(dir.path().join("bad_recording/session.jsonl"), "invalid").unwrap();
    assert_eq!(
        run(dir.path(), &["--seek", "bad_recording", "--at", "1"])
            .status
            .code(),
        Some(2)
    );
    for args in [
        vec!["run", "missing.json"],
        vec!["run", "invalid.json"],
        vec!["run", "assembly.json", "--session", "/dev/null/impossible"],
        vec!["run", "assembly.json", "--replay", "missing"],
    ] {
        assert_eq!(run(dir.path(), &args).status.code(), Some(1), "{args:?}");
    }
    std::fs::write(dir.path().join("guest.wasm"), "invalid wasm").unwrap();
    assert_eq!(
        run(dir.path(), &["run", "assembly.json"]).status.code(),
        Some(1)
    );
}
#[test]
fn demo_shell_exits_cleanly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_fw"))
        .arg("shell")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
    success(child.wait_with_output().unwrap());
}
