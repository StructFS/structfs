//! End-to-end tests: assemblies, wiring, the server protocol, lifecycle,
//! failure policies, the fractal property, and the shell.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use featherweight_runtime::{
    native, protocol, register_builtins, AssemblyDef, BlockState, NativeBlock, Runtime, ShellBlock,
};
use structfs_core_store::{path, Error, Reader, Record, Value, Writer};

fn runtime() -> Runtime {
    let mut runtime = Runtime::new();
    register_builtins(&mut runtime);
    runtime
}

fn base_dir() -> std::path::PathBuf {
    std::env::temp_dir()
}

#[tokio::test(flavor = "multi_thread")]
async fn kv_assembly_serves_reads_and_writes() {
    let runtime = runtime();
    let def = AssemblyDef::from_str(
        r#"{"assembly": "kv_only", "blocks": {"kv": "builtin:kv"}, "public": "kv"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap();

    let written = assembly
        .write(path!("users/alice"), Value::from("Alice"))
        .await
        .unwrap();
    assert_eq!(written, path!("users/alice"));

    let value = assembly.read(path!("users/alice")).await.unwrap();
    assert_eq!(value, Some(Value::from("Alice")));

    // Missing paths are absent, not errors.
    assert_eq!(assembly.read(path!("users/nobody")).await.unwrap(), None);

    assembly.shutdown(Duration::from_secs(2)).await;
    assert_eq!(assembly.public_cell().state(), BlockState::Stopped);
}

/// A block that serves reads by forwarding to its wired kv service —
/// exercises block-to-block calls through namespaces from inside a serve
/// loop.
struct ProxyBlock;

impl NativeBlock for ProxyBlock {
    fn run(&mut self, ns: &mut featherweight_runtime::Namespace) -> Result<(), Error> {
        native::serve_requests(ns, |ns, request| {
            let target = path!("services/kv").join(&request.path);
            match request.op.as_str() {
                "read" => match ns.read(&target) {
                    Ok(Some(record)) => {
                        protocol::ok_value(record.as_value().cloned().unwrap_or(Value::Null))
                    }
                    Ok(None) => protocol::ok_value(Value::Null),
                    Err(e) => protocol::error_to_response(&e),
                },
                "write" => match ns.write(&target, Record::parsed(request.data.clone())) {
                    // Serve the result relative to our own store root.
                    Ok(_) => protocol::ok_path(&request.path),
                    Err(e) => protocol::error_to_response(&e),
                },
                _ => protocol::error_to_response(&Error::store("proxy", "serve", "unknown op")),
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blocks_call_wired_blocks_through_namespaces() {
    let mut runtime = runtime();
    runtime.register_builtin(
        "proxy",
        Arc::new(|| Box::new(ProxyBlock) as Box<dyn NativeBlock>),
    );
    let def = AssemblyDef::from_str(
        r#"{"assembly": "proxied",
            "blocks": {"proxy": "builtin:proxy", "kv": "builtin:kv"},
            "public": "proxy",
            "wiring": ["proxy:/services/kv -> kv"]}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap();

    // kv starts lazily: created until the proxy's first forwarded call.
    assert_eq!(assembly.cell("kv").unwrap().state(), BlockState::Created);

    assembly
        .write(path!("greeting"), Value::from("hello"))
        .await
        .unwrap();
    assert_eq!(
        assembly.read(path!("greeting")).await.unwrap(),
        Some(Value::from("hello"))
    );
    assert_eq!(assembly.cell("kv").unwrap().state(), BlockState::Running);

    assembly.shutdown(Duration::from_secs(2)).await;
    assert_eq!(assembly.cell("kv").unwrap().state(), BlockState::Stopped);
    assert!(assembly.cell("kv").unwrap().shutdown_complete());
}

/// A block that fails immediately.
struct CrashBlock;

impl NativeBlock for CrashBlock {
    fn run(&mut self, _ns: &mut featherweight_runtime::Namespace) -> Result<(), Error> {
        Err(Error::store("crash", "run", "boom"))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn isolate_failure_is_contained() {
    let mut runtime = runtime();
    runtime.register_builtin(
        "crash",
        Arc::new(|| Box::new(CrashBlock) as Box<dyn NativeBlock>),
    );
    let def = AssemblyDef::from_str(
        r#"{"assembly": "iso_fail",
            "blocks": {"kv": "builtin:kv", "crash": "builtin:crash"},
            "public": "kv",
            "failure": {"crash": "isolate"}}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap();

    // Poke the crash block through the host escape hatch: it lazily
    // starts, fails immediately, and the caller sees "unavailable" — no
    // implementation detail leaks.
    let err = assembly
        .read_block("crash", path!("anything"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Overloaded { .. } | Error::DeadlineExceeded { .. }
    ));

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if assembly.cell("crash").unwrap().state() == BlockState::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("crash block never reached Failed");

    // The public block is unaffected (isolate).
    assembly
        .write(path!("still/alive"), Value::from(1i64))
        .await
        .unwrap();
    assert_eq!(assembly.public_cell().state(), BlockState::Running);

    assembly.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fail_fast_takes_down_siblings() {
    let mut runtime = runtime();
    runtime.register_builtin(
        "crash",
        Arc::new(|| Box::new(CrashBlock) as Box<dyn NativeBlock>),
    );
    // The crashing block is public, so it starts eagerly and fails fast.
    let def = AssemblyDef::from_str(
        r#"{"assembly": "ff",
            "blocks": {"crash": "builtin:crash", "kv": "builtin:kv"},
            "public": "crash",
            "wiring": ["crash:/services/kv -> kv"]}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), assembly.wait_public_terminal())
        .await
        .unwrap();
    assert_eq!(assembly.public_cell().state(), BlockState::Failed);
    assert!(assembly
        .public_cell()
        .last_error()
        .unwrap()
        .contains("boom"));

    // Sibling kv never started; fail-fast shutdown moves it to Stopped.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if assembly.cell("kv").unwrap().state() == BlockState::Stopped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("kv sibling was not shut down by fail-fast");

    // Operations on a failed store: unavailable, nothing leaks.
    let err = assembly.read(path!("x")).await.unwrap_err();
    assert!(matches!(err, Error::Overloaded { .. }));
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_assembly_is_a_block() {
    // The fractal property: a nested assembly definition file is used as
    // a block, and calls through the parent reach the child's public kv.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("child.json"),
        r#"{"assembly": "child", "blocks": {"kv": "builtin:kv"}, "public": "kv"}"#,
    )
    .unwrap();

    let mut runtime = runtime();
    runtime.register_builtin(
        "proxy",
        Arc::new(|| Box::new(ProxyBlock) as Box<dyn NativeBlock>),
    );
    let def = AssemblyDef::from_str(
        r#"{"assembly": "parent",
            "blocks": {"proxy": "builtin:proxy", "store": "child.json"},
            "public": "proxy",
            "wiring": ["proxy:/services/kv -> store"]}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), dir.path())
        .unwrap();

    assembly
        .write(path!("nested/value"), Value::from(42i64))
        .await
        .unwrap();
    assert_eq!(
        assembly.read(path!("nested/value")).await.unwrap(),
        Some(Value::Integer(42))
    );

    assembly.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_wire_host_stores() {
    let runtime = runtime();
    let def = AssemblyDef::from_str(
        r#"{"assembly": "imp",
            "blocks": {"proxy": "builtin:kv"},
            "public": "proxy",
            "imports": {"seed": "Seed data"},
            "wiring": ["proxy:/services/seed -> $seed"]}"#,
    )
    .unwrap();

    let mut seed = structfs_core_store::MemoryStore::new();
    seed.write(&path!("motd"), Record::parsed(Value::from("welcome")))
        .unwrap();
    let mut imports = HashMap::new();
    imports.insert("seed".to_string(), featherweight_runtime::host_store(seed));

    let assembly = runtime.instantiate(&def, imports, &base_dir()).unwrap();
    // kv doesn't consult its wiring, but instantiation validated and
    // bound the import; a missing import is an error:
    let err = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap_err();
    assert!(err.to_string().contains("requires import"));

    assembly.shutdown(Duration::from_secs(2)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unresponsive_block_hits_deadline() {
    /// Never reads its requests.
    struct DeafBlock;
    impl NativeBlock for DeafBlock {
        fn run(&mut self, ns: &mut featherweight_runtime::Namespace) -> Result<(), Error> {
            // Park on shutdown instead of serving.
            loop {
                if ns.cell().shutdown_requested() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    let mut runtime = Runtime::new().with_timeout(Duration::from_millis(200));
    register_builtins(&mut runtime);
    runtime.register_builtin(
        "deaf",
        Arc::new(|| Box::new(DeafBlock) as Box<dyn NativeBlock>),
    );
    let def = AssemblyDef::from_str(
        r#"{"assembly": "deaf", "blocks": {"deaf": "builtin:deaf"}, "public": "deaf"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap();

    let err = assembly.read(path!("anything")).await.unwrap_err();
    assert!(matches!(err, Error::DeadlineExceeded { .. }));

    assembly.shutdown(Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_exercises_the_os_surface() {
    let script = "\
id
state
time
uuid
ls
ls iso
ls services
write services/kv/users/alice {\"name\": \"Alice\", \"age\": 30}
read services/kv/users/alice
ls services/kv/users
read services/echo/ping
log info hello from the shell
read iso/self/interface
read missing/path
write unwired/path 1
exit
";
    let output = native::SharedOutput::default();
    let shell_output = output.clone();

    let mut runtime = runtime();
    runtime.register_builtin(
        "test_shell",
        Arc::new(move || {
            Box::new(ShellBlock::with_io(
                Box::new(std::io::Cursor::new(script.as_bytes().to_vec())),
                Box::new(shell_output.clone()),
            )) as Box<dyn NativeBlock>
        }),
    );

    let def = AssemblyDef::from_str(
        r#"{"assembly": "demo",
            "blocks": {"shell": "builtin:test_shell",
                       "kv": "builtin:kv",
                       "echo": "builtin:echo"},
            "public": "shell",
            "wiring": ["shell:/services/kv -> kv", "shell:/services/echo -> echo"],
            "config": {"shell": {"prompt": "iso> "}},
            "failure": {"kv": "isolate"}}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir())
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), assembly.wait_public_terminal())
        .await
        .expect("shell did not finish");
    assembly.shutdown(Duration::from_secs(2)).await;

    let text = output.text();
    // Identity and system paths
    assert!(text.contains("block-"), "id output missing: {text}");
    assert!(text.contains("running"), "state output missing: {text}");
    // ls of the namespace root shows iso + wired mounts
    assert!(text.contains("iso"), "root listing missing iso: {text}");
    assert!(
        text.contains("services"),
        "root listing missing services: {text}"
    );
    // kv round trip
    assert!(
        text.contains("-> services/kv/users/alice"),
        "write echo missing: {text}"
    );
    assert!(
        text.contains("\"name\": \"Alice\""),
        "kv read missing: {text}"
    );
    assert!(text.contains("alice"), "kv ls missing: {text}");
    // echo service
    assert!(
        text.contains("\"echo\": \"ping\""),
        "echo read missing: {text}"
    );
    // interface declaration
    assert!(
        text.contains("interactive shell"),
        "interface missing: {text}"
    );
    // absence and capability denial
    assert!(text.contains("(absent)"), "absent read missing: {text}");
    assert!(
        text.contains("permission denied"),
        "unwired write not denied: {text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn yaml_and_json_definitions_are_equivalent() {
    let yaml =
        AssemblyDef::from_str("assembly: same\nblocks:\n  kv: builtin:kv\npublic: kv\n").unwrap();
    let json = AssemblyDef::from_str(
        r#"{"assembly": "same", "blocks": {"kv": "builtin:kv"}, "public": "kv"}"#,
    )
    .unwrap();
    assert_eq!(yaml, json);
}
