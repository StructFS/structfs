use clap::Parser;

use structfs_repl::EditMode;

/// StructFS - Interactive REPL for StructFS stores
#[derive(Parser, Debug)]
#[command(name = "structfs")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Force vi editing mode
    #[arg(long, conflicts_with = "emacs")]
    vi: bool,

    /// Force emacs editing mode
    #[arg(long)]
    emacs: bool,
}

fn main() {
    let args = Args::parse();

    let edit_mode = if args.vi {
        Some(EditMode::Vi)
    } else if args.emacs {
        Some(EditMode::Emacs)
    } else {
        None
    };

    if let Err(e) = structfs_repl::run(edit_mode) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
