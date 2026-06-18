// xtask — build tooling entry point
// Subcommands: eval, determinism-check, bench (stubs for WP-12)

use anyhow::Result;
use clap::{Parser, Subcommand};

mod eval;

#[derive(Parser)]
#[command(name = "xtask", about = "cgx build tooling")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the fixture eval harness: compare extracted facts against goldens.
    Eval(eval::EvalArgs),
    /// Stub: determinism check (implemented in WP-12).
    DeterminismCheck,
    /// Stub: performance benchmark (implemented in WP-12).
    Bench,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Eval(args) => eval::run(args),
        Commands::DeterminismCheck => {
            eprintln!("determinism-check: not yet implemented (WP-12)");
            Ok(())
        }
        Commands::Bench => {
            eprintln!("bench: not yet implemented (WP-12)");
            Ok(())
        }
    }
}
