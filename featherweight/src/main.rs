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
    host_store, register_builtins, AssemblyDef, Determinism, Runtime, TranscriptMode,
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
  fw run <assembly.json|yaml> [--record DIR | --replay DIR] [--seed N]
                               run an assembly definition
Orthogonal features, mixable freely:
  --record DIR   write each block's boundary answers as a transcript in DIR
  --replay DIR   answer every boundary operation from the transcripts in DIR;
                 the live world is never consulted
  --seed N       deterministic mode: seeded entropy and a virtual clock, so
                 two runs with one seed are the same run";

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
                "--record" => TranscriptMode::Record(transcript_provider(dir, true)),
                _ => TranscriptMode::Replay(transcript_provider(dir, false)),
            };
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
