//! `fw`: the Featherweight Isotope runtime CLI.
//!
//! - `fw shell` runs the built-in demo assembly: an interactive shell
//!   block wired to kv, echo, and logger service blocks.
//! - `fw run <assembly.(json|yaml)>` instantiates an assembly definition
//!   and waits for its public block to finish.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use featherweight_runtime::{
    host_store, register_builtins, AssemblyDef, Determinism, Runtime, SessionEntry, TranscriptMode,
    TranscriptProvider,
};
use structfs_json_store::{JsonlFileBacking, LogStore};

/// The demo: a shell as the public block, with services wired in.
const DEMO_ASSEMBLY: &str = r#"
assembly: fw-demo
version: "0.1.0"

blocks:
  shell:
    artifact: builtin:shell
    stdio: host
    spawn: true
    env:
      DEMO: "1"
    args: ["shell"]
  kv: builtin:kv
  echo: builtin:echo
  logs: builtin:logger

public: shell

wiring:
  - "shell:/services/kv -> kv"
  - "shell:/services/echo -> echo"
  - "shell:/services/logs -> logs"

config:
  shell:
    prompt: "iso> "

failure:
  kv: isolate
"#;

const USAGE: &str = "usage:
  fw shell                     run the demo assembly (interactive shell)
  fw run <assembly.json|yaml> [--record DIR | --replay DIR | --seek DIR [--at SEQ]]
                              [--seed N] [--session FILE]
                               run an assembly definition
Orthogonal features, mixable freely:
  --record DIR    write each block's boundary answers as a transcript in DIR
                  (and a session log to DIR/session.jsonl)
  --replay DIR    answer every boundary operation from the transcripts in DIR;
                  the live world is never consulted
  --seek DIR      replay the transcripts in DIR, then hand off to live
                  execution; --at SEQ stops the replay at that point on
                  DIR/session.jsonl's timeline. Refused if the replayed
                  prefix wrote to a wired peer (that state would be missing
                  live); with --seed, sources fast-forward so the run
                  continues exactly where a straight seeded run would be
  --seed N        deterministic sources: seeded entropy and a virtual clock,
                  so two runs with one seed see the same inputs (blocks still
                  run in parallel at full speed)
  --sim N         full simulation: --seed plus the deterministic scheduler —
                  cross-block interleaving is drawn from the seed too, racy
                  assemblies become one reproducible run per seed, and
                  deadlocks are detected and shut down loudly; blocks run one
                  at a time, so this trades throughput for reproducibility
  --session FILE  forensics: an assembly-wide, arrival-order log of every
                  block's boundary operations — works live, recording, or
                  replaying";

/// Per-block transcripts as stores: one JSONL-backed append log per block in
/// `dir`. The runtime sees only the store; the file is this provider's
/// implementation detail.
///
/// Keys are assembly-scoped paths (`demo/shell`, `demo/sub/inner`,
/// `child#2/kv`), laid out as directories under `dir`. Segments are
/// sanitized so a hostile block name can't escape it.
fn transcript_provider(dir: std::path::PathBuf, fresh: bool) -> Arc<TranscriptProvider> {
    Arc::new(move |block: &str| {
        let mut file = dir.clone();
        for segment in block.split('/') {
            let safe: String = segment
                .chars()
                .map(|c| match c {
                    'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '#' | '.' => c,
                    _ => '_',
                })
                .collect();
            file.push(if safe == ".." { "__".to_string() } else { safe });
        }
        file.set_file_name(format!(
            "{}.transcript.jsonl",
            file.file_name().unwrap_or_default().to_string_lossy()
        ));
        if fresh {
            // A new recording replaces the old transcript; appending to a
            // previous run's entries would corrupt both.
            let _ = std::fs::remove_file(&file);
        } else if !file.exists() {
            return Err(structfs_core_store::Error::store(
                "transcript",
                "replay",
                format!("no transcript for block '{block}' at {}", file.display()),
            ));
        }
        Ok(host_store(LogStore::open(JsonlFileBacking::new(&file))?))
    })
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // --record DIR | --replay DIR (mutually exclusive) and --seed N
    // (orthogonal to both), position-free.
    let mut transcript_mode = TranscriptMode::Off;
    let mut record_dir: Option<std::path::PathBuf> = None;
    let mut seek_dir: Option<std::path::PathBuf> = None;
    for flag in ["--record", "--replay"] {
        if let Some(at) = args.iter().position(|a| a == flag) {
            if at + 1 >= args.len() {
                eprintln!("fw: {flag} needs a directory\n{USAGE}");
                std::process::exit(2);
            }
            if !matches!(transcript_mode, TranscriptMode::Off) {
                eprintln!("fw: --record and --replay are mutually exclusive");
                std::process::exit(2);
            }
            let dir = std::path::PathBuf::from(args.remove(at + 1));
            args.remove(at);
            transcript_mode = match flag {
                "--record" => {
                    record_dir = Some(dir.clone());
                    TranscriptMode::Record(transcript_provider(dir, true))
                }
                _ => TranscriptMode::Replay(transcript_provider(dir, false)),
            };
        }
    }
    // --seek DIR [--at SEQ]: replay a prefix, then continue live.
    if let Some(at) = args.iter().position(|a| a == "--seek") {
        let Some(dir) = args.get(at + 1) else {
            eprintln!("fw: --seek needs a directory\n{USAGE}");
            std::process::exit(2);
        };
        if !matches!(transcript_mode, TranscriptMode::Off) {
            eprintln!("fw: --seek is mutually exclusive with --record/--replay");
            std::process::exit(2);
        }
        seek_dir = Some(std::path::PathBuf::from(dir));
        args.remove(at + 1);
        args.remove(at);
    }
    if let Some(dir) = seek_dir {
        let mut horizon: Option<u64> = None;
        if let Some(at) = args.iter().position(|a| a == "--at") {
            let Some(seq) = args.get(at + 1).and_then(|n| n.parse().ok()) else {
                eprintln!("fw: --at needs a session seq\n{USAGE}");
                std::process::exit(2);
            };
            horizon = Some(seq);
            args.remove(at + 1);
            args.remove(at);
        }
        // A horizon needs the timeline: map session seq to per-block
        // transcript cursors. Without --at, every block replays its
        // whole transcript before going live.
        let to: std::sync::Arc<featherweight_runtime::transcript::SeekPoint> = match horizon {
            None => std::sync::Arc::new(|_: &str| None),
            Some(seq) => {
                let session = dir.join("session.jsonl");
                let text = std::fs::read_to_string(&session).unwrap_or_else(|e| {
                    eprintln!("fw: --at needs {}: {e}", session.display());
                    std::process::exit(2);
                });
                let entries: Vec<SessionEntry> = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(serde_json::from_str)
                    .collect::<Result<_, _>>()
                    .unwrap_or_else(|e| {
                        eprintln!("fw: {} is not a session log: {e}", session.display());
                        std::process::exit(2);
                    });
                let cursors = SessionEntry::cursors_at(&entries, seq);
                std::sync::Arc::new(move |key: &str| Some(cursors.get(key).copied().unwrap_or(0)))
            }
        };
        transcript_mode = TranscriptMode::Seek {
            provider: transcript_provider(dir, false),
            to,
        };
    }

    // --session FILE: explicit wins; --record DIR implies DIR/session.jsonl.
    let mut session_file: Option<std::path::PathBuf> = None;
    if let Some(at) = args.iter().position(|a| a == "--session") {
        let Some(file) = args.get(at + 1) else {
            eprintln!("fw: --session needs a file\n{USAGE}");
            std::process::exit(2);
        };
        session_file = Some(std::path::PathBuf::from(file));
        args.remove(at + 1);
        args.remove(at);
    }
    if session_file.is_none() {
        if let Some(dir) = &record_dir {
            session_file = Some(dir.join("session.jsonl"));
        }
    }

    let mut determinism = Determinism::Live;
    if let Some(at) = args.iter().position(|a| a == "--seed") {
        let Some(seed) = args.get(at + 1).and_then(|n| n.parse().ok()) else {
            eprintln!("fw: --seed needs an integer\n{USAGE}");
            std::process::exit(2);
        };
        args.remove(at + 1);
        args.remove(at);
        determinism = Determinism::Seeded { seed };
    }
    if let Some(at) = args.iter().position(|a| a == "--sim") {
        let Some(seed) = args.get(at + 1).and_then(|n| n.parse().ok()) else {
            eprintln!("fw: --sim needs an integer\n{USAGE}");
            std::process::exit(2);
        };
        if !matches!(determinism, Determinism::Live) {
            eprintln!("fw: --sim and --seed are mutually exclusive (--sim implies --seed)");
            std::process::exit(2);
        }
        args.remove(at + 1);
        args.remove(at);
        determinism = Determinism::Simulation { seed };
    }

    let (source, base_dir) = match args.first().map(String::as_str) {
        Some("shell") => (DEMO_ASSEMBLY.to_string(), std::path::PathBuf::from(".")),
        Some("run") => {
            let Some(file) = args.get(1) else {
                eprintln!("{USAGE}");
                std::process::exit(2);
            };
            let path = std::path::PathBuf::from(file);
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(e) => {
                    eprintln!("fw: cannot read {file}: {e}");
                    std::process::exit(1);
                }
            };
            let base = path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            (source, base)
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };

    let def = match AssemblyDef::from_str(&source) {
        Ok(def) => def,
        Err(e) => {
            eprintln!("fw: {e}");
            std::process::exit(1);
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut runtime = Runtime::with_handle(rt.handle().clone())
        .with_transcripts(transcript_mode)
        .with_determinism(determinism);
    if let Some(file) = session_file {
        // A fresh log per run: a session is one run's timeline.
        let _ = std::fs::remove_file(&file);
        match LogStore::open(JsonlFileBacking::new(&file)) {
            Ok(log) => runtime = runtime.with_session_log(host_store(log)),
            Err(e) => {
                eprintln!("fw: cannot open session log {}: {e}", file.display());
                std::process::exit(1);
            }
        }
    }
    register_builtins(&mut runtime);
    // The WIT component binding is an adapter, not a core concern: the
    // CLI opts in so component artifacts run alongside core modules.
    featherweight_component::register(&mut runtime);

    let assembly = match runtime.instantiate(&def, HashMap::new(), &base_dir) {
        Ok(assembly) => assembly,
        Err(e) => {
            eprintln!("fw: {e}");
            std::process::exit(1);
        }
    };

    rt.block_on(async {
        assembly.wait_public_terminal().await;
        assembly.shutdown(Duration::from_secs(5)).await;
    });

    let public = assembly.public_cell();
    if public.state() == featherweight_runtime::BlockState::Failed {
        eprintln!(
            "fw: assembly '{}' failed: {}",
            assembly.name,
            public.last_error().unwrap_or_default()
        );
        std::process::exit(1);
    }
}
