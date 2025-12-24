use clap::Parser;

/// StructFS - Interactive REPL for StructFS stores
#[derive(Parser, Debug)]
#[command(name = "structfs")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Force vi editing mode
    #[arg(long)]
    vi: bool,

    /// Force emacs editing mode
    #[arg(long)]
    emacs: bool,
}

fn main() {
    let args = Args::parse();

    // Set edit mode override if specified
    if args.vi {
        std::env::set_var("STRUCTFS_EDIT_MODE", "vi");
    } else if args.emacs {
        std::env::set_var("STRUCTFS_EDIT_MODE", "emacs");
    }

    // Run the REPL
    if let Err(e) = structfs_repl::run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
