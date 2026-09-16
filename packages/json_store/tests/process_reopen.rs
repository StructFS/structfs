//! Process termination checks acknowledgement without relying on destructors.
//! This is not a simulation of power loss or filesystem/controller failures.
#![cfg(unix)]
use structfs_json_store::{
    AppendBacking, Backing, Durability, JsonFileBacking, JsonlFileBacking, Value,
};
#[test]
fn acknowledged_files_survive_writer_process_exit() {
    if let Some(path) = std::env::var_os("STRUCTFS_REOPEN_PATH") {
        let mut snapshot = JsonFileBacking::new(std::path::Path::new(&path).join("snapshot"))
            .with_durability(Durability::Synced);
        snapshot.save(&Value::Integer(42)).unwrap();
        let mut log = JsonlFileBacking::new(std::path::Path::new(&path).join("log"))
            .with_durability(Durability::Synced);
        log.append(&Value::Integer(42)).unwrap();
        std::process::exit(23);
    }
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "acknowledged_files_survive_writer_process_exit"])
        .env("STRUCTFS_REOPEN_PATH", dir.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(23));
    assert_eq!(
        JsonFileBacking::new(dir.path().join("snapshot"))
            .load()
            .unwrap(),
        Some(Value::Integer(42))
    );
    assert_eq!(
        JsonlFileBacking::new(dir.path().join("log"))
            .load()
            .unwrap(),
        vec![Value::Integer(42)]
    );
}
