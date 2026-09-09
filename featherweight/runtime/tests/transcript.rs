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

// === Cross-host fixtures ===
//
// The TS browser host speaks the same transcript wire format; the
// fixtures beside its tests pin that byte-for-byte in both directions.
// A transcript recorded by this runtime replays there, and one recorded
// there replays here — the tests below are the "here" half.

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../host/browser/test/fixtures")
}

/// A provider that serves one committed JSONL fixture, whatever the key.
fn fixture_provider(file: std::path::PathBuf) -> Arc<TranscriptProvider> {
    Arc::new(move |_| {
        Ok(host_store(LogStore::open(
            structfs_json_store::JsonlFileBacking::new(&file),
        )?))
    })
}

/// Instantiate the probe guest from the committed artifact and replay
/// it against a fixture transcript.
async fn replay_probe_against(fixture: &str) -> Arc<featherweight_runtime::BlockCell> {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures_dir().join("transcript-probe.wasm"),
        dir.path().join("probe.wasm"),
    )
    .unwrap();
    let runtime = Runtime::new().with_transcripts(TranscriptMode::Replay(fixture_provider(
        fixtures_dir().join(fixture),
    )));
    let def = AssemblyDef::from_str(
        r#"{"assembly": "cross-host", "blocks": {"probe": "probe.wasm"}, "public": "probe"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), dir.path())
        .unwrap();
    assembly.wait_public_terminal().await;
    assembly.shutdown(Duration::from_secs(2)).await;
    assembly.public_cell().clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_committed_rust_fixture_replays_here() {
    let cell = replay_probe_against("rust-recorded.jsonl").await;
    assert_eq!(cell.state(), BlockState::Stopped, "{:?}", cell.last_error());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_committed_js_fixture_replays_here() {
    let cell = replay_probe_against("js-recorded.jsonl").await;
    assert_eq!(cell.state(), BlockState::Stopped, "{:?}", cell.last_error());
}

/// Regenerates the committed cross-host fixtures: assembles the probe
/// wat and records a live run of it. Run by hand when the probe or the
/// wire format changes:
///
///   cargo test -p featherweight-runtime --test transcript -- --ignored regenerate
#[tokio::test(flavor = "multi_thread")]
#[ignore = "regenerates committed fixtures; run by hand"]
async fn regenerate_rust_fixture() {
    let fixtures = fixtures_dir();
    let wasm = wat::parse_file(fixtures.join("transcript-probe.wat")).unwrap();
    std::fs::write(fixtures.join("transcript-probe.wasm"), &wasm).unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("probe.wasm"), &wasm).unwrap();
    let recorded = fixtures.join("rust-recorded.jsonl");
    let _ = std::fs::remove_file(&recorded);
    let runtime =
        Runtime::new().with_transcripts(TranscriptMode::Record(fixture_provider(recorded)));
    let def = AssemblyDef::from_str(
        r#"{"assembly": "cross-host", "blocks": {"probe": "probe.wasm"}, "public": "probe"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), dir.path())
        .unwrap();
    assembly.wait_public_terminal().await;
    assembly.shutdown(Duration::from_secs(2)).await;
    assert_eq!(
        assembly.public_cell().state(),
        BlockState::Stopped,
        "{:?}",
        assembly.public_cell().last_error()
    );
}

// === The session log ===

use featherweight_runtime::SessionEntry;

fn session_store() -> HostStore {
    host_store(LogStore::open(MemoryAppendBacking::new()).unwrap())
}

fn session_entries(store: &HostStore) -> Vec<SessionEntry> {
    let mut store = store.clone();
    let all = store
        .read(&structfs_core_store::path!(""))
        .unwrap()
        .unwrap();
    let Some(Value::Array(items)) = all.as_value() else {
        panic!("expected the session log's array");
    };
    items
        .iter()
        .map(|item| structfs_serde_store::from_value(item.clone()).unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_session_log_witnesses_the_whole_assembly() {
    let (transcripts, provider) = shared_transcripts();
    let session = session_store();

    let seen: Seen = Arc::default();
    let mut runtime = probe_runtime(&seen, TranscriptMode::Record(provider));
    runtime = runtime.with_session_log(session.clone());
    let cell = run_assembly(&runtime, WIRED).await;
    assert_eq!(cell.state(), BlockState::Stopped);

    let entries = session_entries(&session);
    // Arrival order is dense from zero — one witness, one clock.
    assert_eq!(
        entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (0..entries.len() as u64).collect::<Vec<_>>()
    );
    // Both blocks appear under their transcript keys: the probe's calls
    // and the kv service's own mailbox reads interleave in one timeline.
    assert!(entries.iter().any(|e| e.block == "probed/probe"));
    assert!(entries.iter().any(|e| e.block == "probed/kv"));
    // Recording links every entry into its block's transcript; the
    // join holds: the linked transcript entry is the same operation.
    let sample = entries
        .iter()
        .find(|e| e.block == "probed/probe" && e.outcome == "failed:permission_denied")
        .expect("the probe's refusal is witnessed");
    let index = sample.entry.expect("recording links entries");
    let mut probe_log = transcripts.lock().unwrap()["probed/probe"].clone();
    let linked = probe_log
        .read(
            &structfs_core_store::path!("entries")
                .join(&structfs_core_store::Path::parse(&index.to_string()).unwrap()),
        )
        .unwrap()
        .expect("linked transcript entry exists");
    let Some(Value::Map(map)) = linked.as_value() else {
        panic!("transcript entry is a map");
    };
    assert_eq!(map.get("path"), Some(&Value::from(sample.path.to_string())));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_run_flight_records_without_transcripts() {
    let session = session_store();
    let seen: Seen = Arc::default();
    let runtime = probe_runtime(&seen, TranscriptMode::Off).with_session_log(session.clone());
    run_assembly(&runtime, WIRED).await;

    let entries = session_entries(&session);
    assert!(!entries.is_empty());
    // No transcript, no links — the flight recorder stands alone.
    assert!(entries.iter().all(|e| e.entry.is_none()));
    assert!(entries.iter().any(|e| e.outcome == "found"));
    assert!(entries.iter().any(|e| e.outcome == "wrote"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_writes_its_own_session_timeline() {
    let (_transcripts, provider) = shared_transcripts();

    let recorded_session = session_store();
    let seen: Seen = Arc::default();
    let runtime = probe_runtime(&seen, TranscriptMode::Record(provider.clone()))
        .with_session_log(recorded_session.clone());
    run_assembly(&runtime, WIRED).await;

    let replayed_session = session_store();
    let seen: Seen = Arc::default();
    let runtime = probe_runtime(&seen, TranscriptMode::Replay(provider))
        .with_session_log(replayed_session.clone());
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Stopped);

    // The replayed probe's timeline matches the recorded probe's, op
    // for op, outcome for outcome — the forensic view of the claim the
    // transcript already enforces.
    let probe_line = |entries: &[SessionEntry]| -> Vec<(String, String, String)> {
        entries
            .iter()
            .filter(|e| e.block == "probed/probe")
            .map(|e| (e.op.clone(), e.path.to_string(), e.outcome.clone()))
            .collect()
    };
    assert_eq!(
        probe_line(&session_entries(&recorded_session)),
        probe_line(&session_entries(&replayed_session))
    );
}

// === Seek: replay a prefix, then hand off to live execution ===

/// A probe that reads entropy `n` times, capturing what it saw.
struct EntropyProbe {
    n: usize,
    seen: Seen,
}
impl NativeBlock for EntropyProbe {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        for _ in 0..self.n {
            let uuid = ns.read(&path!("iso/random/uuid"))?;
            self.seen
                .lock()
                .unwrap()
                .push(format!("{:?}", uuid.and_then(|r| r.as_value().cloned())));
        }
        Ok(())
    }
}

fn entropy_runtime(
    n: usize,
    seen: &Seen,
    mode: TranscriptMode,
    determinism: Determinism,
) -> Runtime {
    let mut runtime = Runtime::new()
        .with_transcripts(mode)
        .with_determinism(determinism);
    let seen = seen.clone();
    runtime.register_builtin(
        "probe",
        Arc::new(move || {
            Box::new(EntropyProbe {
                n,
                seen: seen.clone(),
            }) as Box<dyn NativeBlock>
        }),
    );
    runtime
}

/// The flagship: replay two of three seeded entropy reads, hand off,
/// and the third — served live by fast-forwarded sources — is exactly
/// what the recorded straight run saw. The seek continues the same run.
#[tokio::test(flavor = "multi_thread")]
async fn a_seek_continues_the_same_seeded_run() {
    let (_transcripts, provider) = shared_transcripts();
    let seeded = Determinism::Seeded { seed: 42 };

    let recorded: Seen = Arc::default();
    let runtime = entropy_runtime(
        3,
        &recorded,
        TranscriptMode::Record(provider.clone()),
        seeded.clone(),
    );
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Stopped);

    let sought: Seen = Arc::default();
    let runtime = entropy_runtime(
        3,
        &sought,
        TranscriptMode::Seek {
            provider,
            to: Arc::new(|_| Some(2)),
        },
        seeded,
    );
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Stopped, "{:?}", cell.last_error());

    let recorded = recorded.lock().unwrap().clone();
    let sought = sought.lock().unwrap().clone();
    // Reads 0 and 1 came from the transcript; read 2 ran live off the
    // fast-forwarded seeded source — and all three match the straight run.
    assert_eq!(recorded, sought);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_seek_hands_off_where_the_transcript_ends() {
    let (_transcripts, provider) = shared_transcripts();

    let recorded: Seen = Arc::default();
    let runtime = entropy_runtime(
        1,
        &recorded,
        TranscriptMode::Record(provider.clone()),
        Determinism::Live,
    );
    run_assembly(&runtime, UNWIRED).await;

    // The continuation asks for more than the recorded run did: a plain
    // replay would fail with "ran out"; a seek goes live instead.
    let sought: Seen = Arc::default();
    let runtime = entropy_runtime(
        2,
        &sought,
        TranscriptMode::Seek {
            provider,
            to: Arc::new(|_| None),
        },
        Determinism::Live,
    );
    let cell = run_assembly(&runtime, UNWIRED).await;
    assert_eq!(cell.state(), BlockState::Stopped, "{:?}", cell.last_error());

    let recorded = recorded.lock().unwrap().clone();
    let sought = sought.lock().unwrap().clone();
    assert_eq!(sought[0], recorded[0], "the prefix replays faithfully");
    assert_ne!(sought[1], recorded[0], "the continuation is live");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seek_past_peer_effects_is_refused() {
    let (_transcripts, provider) = shared_transcripts();

    // The standard probe writes into its wired kv service.
    let seen: Seen = Arc::default();
    let runtime = probe_runtime(&seen, TranscriptMode::Record(provider.clone()));
    run_assembly(&runtime, WIRED).await;

    let seen: Seen = Arc::default();
    let runtime = probe_runtime(
        &seen,
        TranscriptMode::Seek {
            provider,
            to: Arc::new(|_| None),
        },
    );
    let def = AssemblyDef::from_str(UNWIRED).unwrap();
    let refused = runtime
        .instantiate(&def, HashMap::new(), &std::env::temp_dir())
        .map(|_| ())
        .expect_err("a prefix with peer effects must not hand off");
    let message = refused.to_string();
    assert!(message.contains("cannot seek"), "{message}");
    assert!(message.contains("services/kv"), "{message}");
}

/// Record the probe under seed 42 into `file` — the seeded fixture run.
async fn record_seeded_probe(file: std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        fixtures_dir().join("transcript-probe.wasm"),
        dir.path().join("probe.wasm"),
    )
    .unwrap();
    let _ = std::fs::remove_file(&file);
    let runtime = Runtime::new()
        .with_transcripts(TranscriptMode::Record(fixture_provider(file)))
        .with_determinism(Determinism::Seeded { seed: 42 });
    let def = AssemblyDef::from_str(
        r#"{"assembly": "cross-host", "blocks": {"probe": "probe.wasm"}, "public": "probe"}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), dir.path())
        .unwrap();
    assembly.wait_public_terminal().await;
    assembly.shutdown(Duration::from_secs(2)).await;
    assert_eq!(
        assembly.public_cell().state(),
        BlockState::Stopped,
        "{:?}",
        assembly.public_cell().last_error()
    );
}

/// Determinism, pinned by the repository: a seeded run recorded today
/// is byte-identical to the committed fixture recorded when the
/// providers were specified. Any drift in the entropy stream, the
/// virtual clock, uuid construction, digests, or the wire format
/// fails here — and the browser host's tests hold its live seeded run
/// to the same committed bytes.
#[tokio::test(flavor = "multi_thread")]
async fn the_seeded_fixture_is_byte_reproducible() {
    let out = tempfile::tempdir().unwrap();
    let fresh = out.path().join("seeded.jsonl");
    record_seeded_probe(fresh.clone()).await;
    assert_eq!(
        std::fs::read_to_string(&fresh).unwrap(),
        std::fs::read_to_string(fixtures_dir().join("seeded-recorded.jsonl")).unwrap(),
        "the seeded run drifted from the committed fixture"
    );
}

/// Regenerates the seeded cross-host fixture. Run by hand when the
/// provider semantics change — and expect the browser host's fixture
/// test to hold you to the spec when they do.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "regenerates committed fixtures; run by hand"]
async fn regenerate_seeded_fixture() {
    record_seeded_probe(fixtures_dir().join("seeded-recorded.jsonl")).await;
}

/// Block identity is a function of the assembly's shape, not the run:
/// two instantiations answer `iso/self/id` with the same string, which
/// the same-seed-same-run claim requires — an id is an input.
#[tokio::test(flavor = "multi_thread")]
async fn block_ids_are_stable_across_runs() {
    struct IdProbe {
        seen: Seen,
    }
    impl NativeBlock for IdProbe {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            let id = ns.read(&path!("iso/self/id"))?;
            self.seen
                .lock()
                .unwrap()
                .push(format!("{:?}", id.and_then(|r| r.as_value().cloned())));
            Ok(())
        }
    }
    let run = |seen: Seen| async move {
        let mut runtime = Runtime::new();
        runtime.register_builtin(
            "probe",
            Arc::new(move || Box::new(IdProbe { seen: seen.clone() }) as Box<dyn NativeBlock>),
        );
        run_assembly(&runtime, UNWIRED).await;
    };
    let first: Seen = Arc::default();
    run(first.clone()).await;
    let second: Seen = Arc::default();
    run(second.clone()).await;
    let first = first.lock().unwrap().clone();
    assert_eq!(first, second.lock().unwrap().clone());
    assert!(first[0].contains("block:probed/probe"), "{:?}", first[0]);
}

/// Under the virtual clock, `time/after` completes in virtual time:
/// instantly on the wall clock, with virtual now advanced by the span.
#[tokio::test(flavor = "multi_thread")]
async fn seeded_time_after_waits_in_virtual_time() {
    struct Sleeper {
        seen: Seen,
    }
    impl NativeBlock for Sleeper {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            let before = ns.read(&path!("iso/time/now_unix_ns"))?;
            ns.read(&path!("iso/time/after/30000"))?;
            let after = ns.read(&path!("iso/time/now_unix_ns"))?;
            let ns_of = |r: Option<Record>| match r.and_then(|r| r.as_value().cloned()) {
                Some(Value::Integer(ns)) => ns,
                other => panic!("expected ns, got {other:?}"),
            };
            self.seen
                .lock()
                .unwrap()
                .push(format!("{}", ns_of(after) - ns_of(before)));
            Ok(())
        }
    }
    let seen: Seen = Arc::default();
    let mut runtime = Runtime::new().with_determinism(Determinism::Seeded { seed: 7 });
    let captured = seen.clone();
    runtime.register_builtin(
        "probe",
        Arc::new(move || {
            Box::new(Sleeper {
                seen: captured.clone(),
            }) as Box<dyn NativeBlock>
        }),
    );
    let started = std::time::Instant::now();
    run_assembly(&runtime, UNWIRED).await;
    // A thirty-second virtual sleep finishes in well under a second of
    // wall time...
    assert!(started.elapsed() < Duration::from_secs(5));
    // ...and virtual time moved by the span plus the `before` read's
    // own tick (each clock read advances one tick after answering).
    let elapsed: i64 = seen.lock().unwrap()[0].parse().unwrap();
    assert_eq!(elapsed, 30_000_000_000 + 1_000_000);
}

// === Simulation: full determinism from seed, racy assemblies included ===

/// An autonomous block: read-modify-write a shared counter five times.
/// Two of these racing on one store make lost updates — and which
/// updates are lost is a function of the schedule.
struct Contender;
impl NativeBlock for Contender {
    fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
        for _ in 0..5 {
            let seen = match ns.read(&path!("out/counter"))? {
                Some(record) => match record.as_value() {
                    Some(Value::Integer(n)) => *n,
                    _ => 0,
                },
                None => 0,
            };
            ns.write(
                &path!("out/counter"),
                Record::parsed(Value::Integer(seen + 1)),
            )?;
        }
        Ok(())
    }
}

const RACY: &str = r#"{
    "assembly": "racy",
    "blocks": {"w1": "builtin:contender", "w2": "builtin:contender"},
    "public": "w1",
    "imports": {"shared": "the contended store"},
    "wiring": ["w1:/out -> $shared", "w2:/out -> $shared"]
}"#;

/// One simulated run of the racy assembly: the session timeline and the
/// final counter.
async fn racy_run(seed: u64) -> (Vec<(String, String, String)>, i64) {
    let session = session_store();
    let mut runtime = Runtime::new()
        .with_determinism(Determinism::Simulation { seed })
        .with_session_log(session.clone());
    runtime.register_builtin(
        "contender",
        Arc::new(|| Box::new(Contender) as Box<dyn NativeBlock>),
    );
    let shared = host_store(structfs_core_store::MemoryStore::new());
    let def = AssemblyDef::from_str(RACY).unwrap();
    let assembly = runtime
        .instantiate(
            &def,
            HashMap::from([(String::from("shared"), shared.clone())]),
            &std::env::temp_dir(),
        )
        .unwrap();
    for name in ["w1", "w2"] {
        tokio::time::timeout(
            Duration::from_secs(10),
            assembly.cell(name).unwrap().wait_terminal(),
        )
        .await
        .unwrap_or_else(|_| panic!("{name} did not finish"));
    }
    assembly.shutdown(Duration::from_secs(2)).await;

    let timeline: Vec<(String, String, String)> = session_entries(&session)
        .into_iter()
        .map(|e| (e.block, e.op, e.path.to_string()))
        .collect();
    let mut shared = shared;
    let counter = match shared
        .read(&structfs_core_store::path!("counter"))
        .unwrap()
        .and_then(|r| r.as_value().cloned())
    {
        Some(Value::Integer(n)) => n,
        other => panic!("expected a counter, got {other:?}"),
    };
    (timeline, counter)
}

/// The Antithesis claim: a racy assembly is one reproducible run per
/// seed — the interleaving, the lost updates, the whole timeline — and
/// a different seed explores a different schedule.
#[tokio::test(flavor = "multi_thread")]
async fn simulation_makes_racy_assemblies_a_function_of_the_seed() {
    let (timeline_a, counter_a) = racy_run(42).await;
    let (timeline_b, counter_b) = racy_run(42).await;
    assert_eq!(timeline_a, timeline_b, "same seed, same interleaving");
    assert_eq!(counter_a, counter_b, "same seed, same lost updates");

    let (timeline_c, _) = racy_run(7).await;
    assert_ne!(
        timeline_a, timeline_c,
        "a different seed explores a different schedule"
    );
}

/// A real deadlock — a dependency cycle — is detected and shut down
/// loudly rather than hanging. Two blocks each call the other on their
/// first turn: whoever the seed runs first calls its peer and parks;
/// the peer, still mid-call, never reads its own mailbox, so it too
/// parks on its call. Nothing is runnable and both are call-parked —
/// the cycle — so the turnstile shuts the assembly down.
///
/// (An idle server parked on its mailbox with no clients left is *not*
/// this: that is quiescence, revived by a host poke or reaped by
/// shutdown — exercised by the nested tests below, whose depots outlive
/// their clients and are still read by the host.)
#[tokio::test(flavor = "multi_thread")]
async fn simulation_detects_a_dependency_cycle() {
    struct Caller {
        peer: &'static str,
    }
    impl NativeBlock for Caller {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            // Call the peer before ever serving: a mutual call cycle.
            let target = structfs_core_store::Path::parse(&format!("peer/{}", self.peer)).unwrap();
            let _ = ns.read(&target);
            Ok(())
        }
    }
    let mut runtime = Runtime::new().with_determinism(Determinism::Simulation { seed: 3 });
    runtime.register_builtin(
        "ping",
        Arc::new(|| Box::new(Caller { peer: "who" }) as Box<dyn NativeBlock>),
    );
    runtime.register_builtin(
        "pong",
        Arc::new(|| Box::new(Caller { peer: "who" }) as Box<dyn NativeBlock>),
    );
    let def = AssemblyDef::from_str(
        r#"{"assembly": "cycle",
            "blocks": {"ping": "builtin:ping", "pong": "builtin:pong"},
            "public": "ping",
            "wiring": ["ping:/peer -> pong", "pong:/peer -> ping"]}"#,
    )
    .unwrap();
    let assembly = runtime
        .instantiate(&def, HashMap::new(), &std::env::temp_dir())
        .unwrap();
    for name in ["ping", "pong"] {
        tokio::time::timeout(
            Duration::from_secs(10),
            assembly.cell(name).unwrap().wait_terminal(),
        )
        .await
        .unwrap_or_else(|_| panic!("{name} still running: the deadlock was not detected"));
    }
}

// === Simulation across nested assemblies ===

/// Read-modify-write a counter at `prefix` five times through whatever
/// is wired there — here, a nested assembly's public store, so every
/// step is a cross-assembly server-protocol call.
fn rmw(ns: &mut Namespace, counter: &structfs_core_store::Path) -> Result<i64, Error> {
    let seen = match ns.read(counter)? {
        Some(record) => match record.as_value() {
            Some(Value::Integer(n)) => *n,
            _ => 0,
        },
        None => 0,
    };
    ns.write(counter, Record::parsed(Value::Integer(seen + 1)))?;
    Ok(seen)
}

/// Write the nested `depot.yaml` definition used by these tests.
fn depot_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("depot.yaml"),
        "assembly: depot\nblocks:\n  kv: builtin:kv\npublic: kv\n",
    )
    .unwrap();
    dir
}

type Timeline = Vec<(String, String, String)>;

fn timeline_of(session: &HostStore) -> Timeline {
    session_entries(session)
        .into_iter()
        .map(|e| (e.block, e.op, e.path.to_string()))
        .collect()
}

/// Two parent blocks race read-modify-writes into one *nested*
/// assembly's store: every contended step crosses the nesting boundary
/// through the server protocol, and the whole tree is one seeded run.
#[tokio::test(flavor = "multi_thread")]
async fn simulation_pins_races_through_a_nested_assembly() {
    struct Chatter;
    impl NativeBlock for Chatter {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            for _ in 0..5 {
                rmw(ns, &path!("services/depot/counter"))?;
            }
            Ok(())
        }
    }

    async fn run(seed: u64) -> (Timeline, Option<Value>) {
        let dir = depot_dir();
        let session = session_store();
        let mut runtime = Runtime::new()
            .with_determinism(Determinism::Simulation { seed })
            .with_session_log(session.clone());
        register_builtins(&mut runtime);
        runtime.register_builtin(
            "chatter",
            Arc::new(|| Box::new(Chatter) as Box<dyn NativeBlock>),
        );
        let def = AssemblyDef::from_str(
            r#"{"assembly": "plaza",
                "blocks": {"c1": "builtin:chatter", "c2": "builtin:chatter",
                           "depot": "depot.yaml"},
                "public": "c1",
                "wiring": ["c1:/services/depot -> depot",
                           "c2:/services/depot -> depot"]}"#,
        )
        .unwrap();
        let assembly = runtime
            .instantiate(&def, HashMap::new(), dir.path())
            .unwrap();
        for name in ["c1", "c2"] {
            tokio::time::timeout(
                Duration::from_secs(10),
                assembly.cell(name).unwrap().wait_terminal(),
            )
            .await
            .unwrap_or_else(|_| panic!("{name} did not finish"));
        }
        // Snapshot the timeline before any host-driven read muddies it.
        let timeline = timeline_of(&session);
        let counter = assembly
            .read_block("depot", path!("counter"))
            .await
            .unwrap();
        assembly.shutdown(Duration::from_secs(2)).await;
        (timeline, counter)
    }

    let (timeline_a, counter_a) = run(42).await;
    let (timeline_b, counter_b) = run(42).await;
    assert_eq!(timeline_a, timeline_b, "same seed, same tree-wide run");
    assert_eq!(counter_a, counter_b, "same seed, same lost updates");
    // The nested block's own turns are on the one timeline, under its
    // tree-scoped key.
    assert!(
        timeline_a
            .iter()
            .any(|(block, _, _)| block == "plaza/depot/kv"),
        "{timeline_a:?}"
    );
    let (timeline_c, _) = run(7).await;
    assert_ne!(timeline_a, timeline_c, "a different seed, another schedule");
}

/// Two nested assemblies talking through racing parent couriers: each
/// courier read-modify-writes depot A and records what it saw into
/// depot B. Interleaving decides both the lost updates in A and the
/// values ferried into B — and all of it is a function of the seed.
#[tokio::test(flavor = "multi_thread")]
async fn simulation_pins_two_nested_assemblies_bridged_by_couriers() {
    struct Courier {
        tag: &'static str,
    }
    impl NativeBlock for Courier {
        fn run(&mut self, ns: &mut Namespace) -> Result<(), Error> {
            for i in 0..4 {
                let seen = rmw(ns, &path!("a/counter"))?;
                let record =
                    structfs_core_store::Path::parse(&format!("b/from_{}_{i}", self.tag)).unwrap();
                ns.write(&record, Record::parsed(Value::Integer(seen)))?;
            }
            Ok(())
        }
    }

    async fn run(seed: u64) -> (Timeline, Option<Value>, Option<Value>) {
        let dir = depot_dir();
        let session = session_store();
        let mut runtime = Runtime::new()
            .with_determinism(Determinism::Simulation { seed })
            .with_session_log(session.clone());
        register_builtins(&mut runtime);
        runtime.register_builtin(
            "courier1",
            Arc::new(|| Box::new(Courier { tag: "k1" }) as Box<dyn NativeBlock>),
        );
        runtime.register_builtin(
            "courier2",
            Arc::new(|| Box::new(Courier { tag: "k2" }) as Box<dyn NativeBlock>),
        );
        let def = AssemblyDef::from_str(
            r#"{"assembly": "bridge",
                "blocks": {"k1": "builtin:courier1", "k2": "builtin:courier2",
                           "a": "depot.yaml", "b": "depot.yaml"},
                "public": "k1",
                "wiring": ["k1:/a -> a", "k1:/b -> b",
                           "k2:/a -> a", "k2:/b -> b"]}"#,
        )
        .unwrap();
        let assembly = runtime
            .instantiate(&def, HashMap::new(), dir.path())
            .unwrap();
        for name in ["k1", "k2"] {
            tokio::time::timeout(
                Duration::from_secs(10),
                assembly.cell(name).unwrap().wait_terminal(),
            )
            .await
            .unwrap_or_else(|_| panic!("{name} did not finish"));
        }
        let timeline = timeline_of(&session);
        let ledger = assembly.read_block("b", path!("")).await.unwrap();
        let counter = assembly.read_block("a", path!("counter")).await.unwrap();
        assembly.shutdown(Duration::from_secs(2)).await;
        (timeline, ledger, counter)
    }

    let (timeline_a, ledger_a, counter_a) = run(42).await;
    let (timeline_b, ledger_b, counter_b) = run(42).await;
    assert_eq!(timeline_a, timeline_b, "same seed, same tree-wide run");
    assert_eq!(ledger_a, ledger_b, "same seed, same ferried values");
    assert_eq!(counter_a, counter_b);
    // Both nested assemblies' blocks share the one seeded timeline,
    // each under its own tree-scoped identity.
    for key in ["bridge/a/kv", "bridge/b/kv", "bridge/k1", "bridge/k2"] {
        assert!(
            timeline_a.iter().any(|(block, _, _)| block == key),
            "missing {key} in {timeline_a:?}"
        );
    }
    let (timeline_c, _, _) = run(7).await;
    assert_ne!(timeline_a, timeline_c, "a different seed, another schedule");
}
