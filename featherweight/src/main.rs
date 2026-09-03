//! `fw`: the Featherweight Isotope runtime CLI.
//!
//! - `fw shell` runs the built-in demo assembly: an interactive shell
//!   block wired to kv, echo, and logger service blocks.
//! - `fw run <assembly.(json|yaml)>` instantiates an assembly definition
//!   and waits for its public block to finish.

use std::collections::HashMap;
use std::time::Duration;

use featherweight_runtime::{register_builtins, AssemblyDef, Runtime};

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
  fw run <assembly.json|yaml>  run an assembly definition";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    let mut runtime = Runtime::with_handle(rt.handle().clone());
    register_builtins(&mut runtime);

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
