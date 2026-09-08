//! Transcripts and determinism end to end (spec 12): a recorded run
//! replays byte-identically with the live world absent, a replay that
//! asks a different question fails loudly, and the two features mix —
//! seeded runs transcribe identically, which is the transcript proving
//! the determinism.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use featherweight_runtime::{
    host_store, register_builtins, AssemblyDef, BlockState, Determinism, HostStore, Namespace,
    NativeBlock, Runtime, TranscriptMode, TranscriptProvider,
};
use structfs_core_store::{path, Error, Reader, Record, Value, Writer};
use structfs_json_store::{LogStore, MemoryAppendBacking};

/// What one run of the probe saw, as strings: every answer, including
/// refusals — the sequence a replay must reproduce exactly.
type Seen = Arc<Mutex<Vec<String>>>;

/// A block that touches every kind of boundary answer: an input read
/// (entropy — impossible to reproduce live), a write acknowledgement, a
/// found read, an absent read, and a refusal.
struct Probe {
    seen: Seen,
}

impl Probe {
    fn note(&self, label: &str, rendered: impl std::fmt::Display) {
        self.seen
            .lock()
            .unwrap()
            .push(format!("{label}: {rendered}"));
    }

    fn note_read(&self, label: &str, result: Result<Option<Record>, Error>) {
        match result {
            Ok(Some(record)) => match record.as_value() {
                Some(value) => self.note(label, format!("{value:?}")),
                None => self.note(label, "raw"),
            },
            Ok(None) => self.note(label, "absent"),
            Err(error) => self.note(label, format!("error: {error}")),
        }
    }
}

impl NativeBlock for Probe {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        self.note_read("uuid", ns.read(&path!("iso/random/uuid")));
        match ns.write(
            &path!("services/kv/greeting"),
            Record::parsed(Value::from("hello")),
        ) {
            Ok(at) => self.note("wrote", at),
            Err(error) => self.note("wrote", format!("error: {error}")),
        }
        self.note_read("greeting", ns.read(&path!("services/kv/greeting")));
        self.note_read("missing", ns.read(&path!("services/kv/missing")));
        self.note_read("unwired", ns.read(&path!("services/nothing")));
        Ok(())
    }
}

/// Transcripts shared between the recording and replaying runtimes: stores in
/// memory, keyed by the runtime's assembly-scoped transcript keys — the
/// transcript never touches a disk here, which is the point of it being
/// a store.
fn shared_transcripts() -> (
    Arc<Mutex<HashMap<String, HostStore>>>,
    Arc<TranscriptProvider>,
) {
    let transcripts: Arc<Mutex<HashMap<String, HostStore>>> = Arc::default();
    let provider: Arc<TranscriptProvider> = {
        let transcripts = transcripts.clone();
        Arc::new(move |name: &str| {
            Ok(transcripts
                .lock()
                .unwrap()
                .entry(name.to_string())
                .or_insert_with(|| host_store(LogStore::open(MemoryAppendBacking::new()).unwrap()))
                .clone())
        })
    };
    (transcripts, provider)
}

fn probe_runtime(seen: &Seen, transcript_mode: TranscriptMode) -> Runtime {
    let mut runtime = Runtime::new().with_transcripts(transcript_mode);
    register_builtins(&mut runtime);
    let seen = seen.clone();
    runtime.register_builtin(
        "probe",
        Arc::new(move || Box::new(Probe { seen: seen.clone() }) as Box<dyn NativeBlock>),
    );
    runtime
}

async fn run_assembly(runtime: &Runtime, def: &str) -> Arc<featherweight_runtime::BlockCell> {
    let def = AssemblyDef::from_str(def).unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &std::env::temp_dir())
        .unwrap();
    assembly.wait_public_terminal().await;
    assembly.shutdown(Duration::from_secs(2)).await;
    assembly.public_cell().clone()
}

// Record and replay definitions share one assembly name: transcript keys
// are assembly-scoped (`probed/probe`), and a replay must present the
// same identity the recording had.
const WIRED: &str = r#"{
    "assembly": "probed",
    "blocks": {"probe": "builtin:probe", "kv": "builtin:kv"},
    "public": "probe",
    "wiring": ["probe:/services/kv -> kv"]
}"#;

/// The replay assembly has no kv block and no wiring at all: the transcript is
/// the world.
const UNWIRED: &str = r#"{
    "assembly": "probed",
    "blocks": {"probe": "builtin:probe"},
    "public": "probe"
}"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_recorded_run_replays_identically_with_the_world_absent() {
    let (_transcripts, provider) = shared_transcripts();

    let recorded: Seen = Arc::default();
    let runtime = probe_runtime(&recorded, TranscriptMode::Record(provider.clone()));
    let cell = run_assembly(&runtime, WIRED).await;
    assert_eq!(cell.state(), BlockState::Stopped);

    let replayed: Seen = Arc::default();
    let runtime = probe_runtime(&replayed, TranscriptMode::Replay(provider));
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(
        cell.state(),
        BlockState::Stopped,
        "replay failed: {:?}",
        cell.last_error()
    );

    let recorded = recorded.lock().unwrap().clone();
    let replayed = replayed.lock().unwrap().clone();
    assert_eq!(recorded, replayed);
    // The entropy answer really came from the transcript: a live run could not
    // have produced the same uuid.
    assert!(recorded[0].starts_with("uuid: String"), "{:?}", recorded[0]);
    // The refusal replayed as the same typed error.
    assert!(
        replayed.last().unwrap().contains("permission denied"),
        "{:?}",
        replayed.last()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_that_asks_a_different_question_diverges_loudly() {
    let (_transcripts, provider) = shared_transcripts();

    let seen: Seen = Arc::default();
    let runtime = probe_runtime(&seen, TranscriptMode::Record(provider.clone()));
    run_assembly(&runtime, WIRED).await;

    // Same block name, different first question.
    struct Impostor;
    impl NativeBlock for Impostor {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.read(&path!("iso/time/now"))?;
            Ok(())
        }
    }
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Replay(provider));
    register_builtins(&mut runtime);
    runtime.register_builtin(
        "probe",
        Arc::new(|| Box::new(Impostor) as Box<dyn NativeBlock>),
    );
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Failed);
    let error = cell.last_error().unwrap_or_default();
    assert!(error.contains("diverged"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_that_asks_more_than_the_transcript_holds_runs_out() {
    let (_transcripts, provider) = shared_transcripts();

    // Record a probe that asks once.
    struct Once;
    impl NativeBlock for Once {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.read(&path!("iso/random/uuid"))?;
            Ok(())
        }
    }
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Record(provider.clone()));
    runtime.register_builtin("probe", Arc::new(|| Box::new(Once) as Box<dyn NativeBlock>));
    run_assembly(&runtime, UNWIRED).await;

    // Replay a probe that asks twice.
    struct Twice;
    impl NativeBlock for Twice {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.read(&path!("iso/random/uuid"))?;
            ns.read(&path!("iso/random/uuid"))?;
            Ok(())
        }
    }
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Replay(provider));
    runtime.register_builtin(
        "probe",
        Arc::new(|| Box::new(Twice) as Box<dyn NativeBlock>),
    );
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Failed);
    let error = cell.last_error().unwrap_or_default();
    assert!(error.contains("ran out"), "{error}");
}

/// The orthogonality claim, exercised: determinism (a seed) and
/// transcription (recording) mix. Two live runs with one seed leave
/// identical transcripts — the transcript is the instrument that proves
/// the determinism — and a different seed leaves a different one.
#[tokio::test(flavor = "multi_thread")]
async fn seeded_runs_transcribe_identically() {
    struct Sensors;
    impl NativeBlock for Sensors {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.read(&path!("iso/random/uuid"))?;
            ns.read(&path!("iso/time/now_unix_ns"))?;
            ns.read(&path!("iso/random/bytes/8"))?;
            ns.read(&path!("iso/time/now"))?;
            Ok(())
        }
    }

    let transcript_of = |seed: u64| async move {
        let (_, provider) = shared_transcripts();
        let mut runtime = Runtime::new()
            .with_transcripts(TranscriptMode::Record(provider.clone()))
            .with_determinism(Determinism::Seeded { seed });
        runtime.register_builtin(
            "probe",
            Arc::new(|| Box::new(Sensors) as Box<dyn NativeBlock>),
        );
        let cell = run_assembly(&runtime, UNWIRED).await;
        assert_eq!(cell.state(), BlockState::Stopped);
        let mut store = provider("probed/probe").unwrap();
        store
            .read(&path!(""))
            .unwrap()
            .unwrap()
            .into_value(&structfs_core_store::NoCodec)
            .unwrap()
    };

    let first = transcript_of(42).await;
    let second = transcript_of(42).await;
    let other = transcript_of(7).await;
    assert_eq!(first, second);
    assert_ne!(first, other);
}

/// A spawner that spawns the same child definition twice — the transcript
/// keys must not collide.
struct TwinSpawner;
impl NativeBlock for TwinSpawner {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        let def = structfs_serde_store::json_to_value(serde_json::json!({
            "assembly": "twin",
            "blocks": {"kv": "builtin:kv"},
            "public": "kv"
        }));
        let first = ns.write(&path!("iso/proc"), Record::parsed(def.clone()))?;
        let second = ns.write(&path!("iso/proc"), Record::parsed(def))?;
        // Use both children so their blocks actually start and get keys.
        ns.write(
            &first.join(&path!("store/x")),
            Record::parsed(Value::from("1")),
        )?;
        ns.write(
            &second.join(&path!("store/x")),
            Record::parsed(Value::from("2")),
        )?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn transcript_keys_scope_assemblies_and_disambiguate_spawns() {
    let (transcripts, provider) = shared_transcripts();
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Record(provider.clone()));
    register_builtins(&mut runtime);
    runtime.register_builtin(
        "boss",
        Arc::new(|| Box::new(TwinSpawner) as Box<dyn NativeBlock>),
    );
    let cell = run_assembly(
        &runtime,
        r#"{"assembly": "spawning",
            "blocks": {"boss": {"artifact": "builtin:boss", "spawn": true}},
            "public": "boss"}"#,
    )
    .await;
    assert_eq!(cell.state(), BlockState::Stopped, "{:?}", cell.last_error());

    let keys: std::collections::BTreeSet<String> =
        transcripts.lock().unwrap().keys().cloned().collect();
    assert!(keys.contains("spawning/boss"), "{keys:?}");
    assert!(keys.contains("twin/kv"), "{keys:?}");
    assert!(keys.contains("twin/kv#2"), "{keys:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_assembly_blocks_get_scoped_transcript_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("child.yaml"),
        "assembly: childdef\nblocks:\n  kv: builtin:kv\npublic: kv\n",
    )
    .unwrap();

    // The probe forces the nested kv to start by writing through it.
    struct Toucher;
    impl NativeBlock for Toucher {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.write(&path!("services/inner/x"), Record::parsed(Value::from("1")))?;
            Ok(())
        }
    }
    let (transcripts, provider) = shared_transcripts();
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Record(provider));
    register_builtins(&mut runtime);
    runtime.register_builtin(
        "toucher",
        Arc::new(|| Box::new(Toucher) as Box<dyn NativeBlock>),
    );
    let def = AssemblyDef::from_str(
        r#"{"assembly": "parent",
            "blocks": {"toucher": "builtin:toucher", "inner": "child.yaml"},
            "public": "toucher",
            "wiring": ["toucher:/services/inner -> inner"]}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), dir.path())
        .unwrap();
    assembly.wait_public_terminal().await;
    assembly.shutdown(Duration::from_secs(2)).await;

    let keys: std::collections::BTreeSet<String> =
        transcripts.lock().unwrap().keys().cloned().collect();
    // The nested block's key is its position in the tree, not its
    // definition's name.
    assert!(keys.contains("parent/toucher"), "{keys:?}");
    assert!(keys.contains("parent/inner/kv"), "{keys:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_that_writes_different_data_diverges() {
    let (_transcripts, provider) = shared_transcripts();

    struct Honest;
    impl NativeBlock for Honest {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.write(&path!("iso/log/info"), Record::parsed(Value::from("hello")))?;
            Ok(())
        }
    }
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Record(provider.clone()));
    runtime.register_builtin(
        "probe",
        Arc::new(|| Box::new(Honest) as Box<dyn NativeBlock>),
    );
    run_assembly(&runtime, UNWIRED).await;

    // Same path, different payload: an acknowledged lie, unless caught.
    struct Liar;
    impl NativeBlock for Liar {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            ns.write(
                &path!("iso/log/info"),
                Record::parsed(Value::from("goodbye")),
            )?;
            Ok(())
        }
    }
    let mut runtime = Runtime::new().with_transcripts(TranscriptMode::Replay(provider));
    runtime.register_builtin("probe", Arc::new(|| Box::new(Liar) as Box<dyn NativeBlock>));
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Failed);
    let error = cell.last_error().unwrap_or_default();
    assert!(error.contains("different data"), "{error}");
}
