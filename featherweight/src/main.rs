//! `fw`: the Featherweight Isotope runtime CLI.
//!
//! - `fw shell` runs the built-in demo assembly: an interactive shell
//!   block wired to kv, echo, and logger service blocks.
//! - `fw run <assembly.(json|yaml)>` instantiates an assembly definition
//!   and waits for its public block to finish.
//!
//! Recording, replay, seek, determinism, and the session log are
//! orthogonal flags accepted before or after the subcommand.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use featherweight_runtime::{
    host_store, register_builtins, AssemblyDef, Determinism, Runtime, RuntimeConfig, SessionEntry,
    TranscriptMode, TranscriptProvider,
};
use structfs_core_store::{path, Reader as _};
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

#[derive(Parser)]
#[command(
    name = "fw",
    version,
    about = "The Featherweight Isotope runtime",
    arg_required_else_help = true,
    after_help = "The transcript, determinism and session flags are orthogonal and mix freely."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[command(flatten)]
    modes: Modes,
}

#[derive(Subcommand)]
enum Command {
    /// Run the demo assembly (an interactive shell wired to kv, echo, logs).
    Shell,
    /// Run an assembly definition (JSON or YAML) until its public block ends.
    Run {
        /// The assembly definition file.
        assembly: PathBuf,
    },
}

#[derive(Args)]
struct Modes {
    /// Write each block's boundary answers as a transcript in DIR (and a
    /// session log to DIR/session.jsonl).
    #[arg(long, value_name = "DIR", global = true, conflicts_with_all = ["replay", "seek"])]
    record: Option<PathBuf>,

    /// Answer every boundary operation from the transcripts in DIR; the
    /// live world is never consulted.
    #[arg(long, value_name = "DIR", global = true, conflicts_with = "seek")]
    replay: Option<PathBuf>,

    /// Replay the transcripts in DIR, then hand off to live execution.
    /// Refused if the replayed prefix wrote to a wired peer (that state
    /// would be missing live); with --seed, sources fast-forward so the run
    /// continues exactly where a straight seeded run would be.
    #[arg(long, value_name = "DIR", global = true)]
    seek: Option<PathBuf>,

    /// With --seek: stop the replay at SEQ on DIR/session.jsonl's timeline.
    #[arg(long, value_name = "SEQ", global = true, requires = "seek")]
    at: Option<u64>,

    /// Deterministic sources: seeded entropy and a virtual clock, so two
    /// runs with one seed see the same inputs (blocks still run in
    /// parallel at full speed).
    #[arg(long, value_name = "N", global = true, conflicts_with = "sim")]
    seed: Option<u64>,

    /// Full simulation: --seed plus the deterministic scheduler — cross-block
    /// interleaving is drawn from the seed too, racy assemblies become one
    /// reproducible run per seed, and deadlocks are detected and shut down
    /// loudly; blocks run one at a time.
    #[arg(long, value_name = "N", global = true)]
    sim: Option<u64>,

    /// Forensics: an assembly-wide, arrival-order log of every block's
    /// boundary operations — works live, recording, or replaying.
    #[arg(long, value_name = "FILE", global = true)]
    session: Option<PathBuf>,
}

/// Exit with a usage error (code 2).
fn usage_error(message: impl std::fmt::Display) -> ! {
    eprintln!("fw: {message}");
    std::process::exit(2);
}

/// Exit with a run failure (code 1).
fn failure(message: impl std::fmt::Display) -> ! {
    eprintln!("fw: {message}");
    std::process::exit(1);
}

/// Per-block transcripts as stores: one JSONL-backed append log per block in
/// `dir`. The runtime sees only the store; the file is this provider's
/// implementation detail.
///
/// Keys are assembly-scoped paths (`demo/shell`, `demo/sub/inner`,
/// `child#2/kv`), laid out as directories under `dir`. Segments are
/// sanitized so a hostile block name can't escape it.
fn transcript_provider(dir: PathBuf, fresh: bool) -> Arc<TranscriptProvider> {
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

/// Per-block seek horizons for "the state just after session `seq`",
/// read from `dir`'s session log.
fn seek_horizons(
    dir: &std::path::Path,
    seq: u64,
) -> Arc<featherweight_runtime::transcript::SeekPoint> {
    let session = dir.join("session.jsonl");
    if !session.exists() {
        usage_error(format!("--at needs {}", session.display()));
    }
    let entries = LogStore::open(JsonlFileBacking::new(&session))
        .and_then(|mut log| log.read(&path!("")))
        .and_then(
            |all| match all.map(|r| r.into_value(&structfs_core_store::NoCodec)) {
                Some(Ok(structfs_core_store::Value::Array(items))) => items
                    .into_iter()
                    .map(structfs_serde_store::from_value::<SessionEntry>)
                    .collect(),
                Some(Err(e)) => Err(e),
                _ => Ok(Vec::new()),
            },
        )
        .unwrap_or_else(|e| {
            usage_error(format!("{} is not a session log: {e}", session.display()))
        });
    let cursors = SessionEntry::cursors_at(&entries, seq);
    Arc::new(move |key: &str| Some(cursors.get(key).copied().unwrap_or(0)))
}

fn transcript_mode(modes: &Modes) -> TranscriptMode {
    if let Some(dir) = &modes.record {
        return TranscriptMode::Record(transcript_provider(dir.clone(), true));
    }
    if let Some(dir) = &modes.replay {
        return TranscriptMode::Replay(transcript_provider(dir.clone(), false));
    }
    if let Some(dir) = &modes.seek {
        // Without --at, every block replays its whole transcript before
        // going live.
        let to = match modes.at {
            None => Arc::new(|_: &str| None) as Arc<featherweight_runtime::transcript::SeekPoint>,
            Some(seq) => seek_horizons(dir, seq),
        };
        return TranscriptMode::Seek {
            provider: transcript_provider(dir.clone(), false),
            to,
        };
    }
    TranscriptMode::Off
}

fn main() {
    let cli = Cli::parse();
    let modes = &cli.modes;

    let transcripts = transcript_mode(modes);
    let determinism = match (modes.seed, modes.sim) {
        (Some(seed), _) => Determinism::Seeded { seed },
        (_, Some(seed)) => Determinism::Simulation { seed },
        _ => Determinism::Live,
    };
    // --session FILE: explicit wins; --record DIR implies DIR/session.jsonl.
    let session_file = modes
        .session
        .clone()
        .or_else(|| modes.record.as_ref().map(|dir| dir.join("session.jsonl")));

    let (source, base_dir) = match &cli.command {
        Command::Shell => (DEMO_ASSEMBLY.to_string(), PathBuf::from(".")),
        Command::Run { assembly } => {
            let source = std::fs::read_to_string(assembly)
                .unwrap_or_else(|e| failure(format!("cannot read {}: {e}", assembly.display())));
            let base = assembly
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            (source, base)
        }
    };
    let def = AssemblyDef::from_str(&source).unwrap_or_else(|e| failure(e));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| failure(format!("cannot start the async runtime: {e}")));
    let mut config = RuntimeConfig::new(rt.handle().clone())
        .with_transcripts(transcripts)
        .with_determinism(determinism);
    if let Some(file) = session_file {
        // A fresh log per run: a session is one run's timeline.
        let _ = std::fs::remove_file(&file);
        match LogStore::open(JsonlFileBacking::new(&file)) {
            Ok(log) => config = config.with_session_log(host_store(log)),
            Err(e) => failure(format!("cannot open session log {}: {e}", file.display())),
        }
    }
    register_builtins(&mut config);
    // The WIT component binding is an adapter, not a core concern: the
    // CLI opts in so component artifacts run alongside core modules.
    featherweight_component::register(&mut config);
    let runtime = Runtime::new(config);

    let assembly = runtime
        .instantiate(&def, HashMap::new(), &base_dir)
        .unwrap_or_else(|e| failure(e));

    rt.block_on(async {
        assembly.wait_public_terminal().await;
        assembly.shutdown(Duration::from_secs(5)).await;
    });

    let public = assembly.public_cell();
    if public.state() == featherweight_runtime::BlockState::Failed {
        failure(format!(
            "assembly '{}' failed: {}",
            assembly.name,
            public.last_error().unwrap_or_default()
        ));
    }
}
