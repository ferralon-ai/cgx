// xtask — build tooling entry point
// Subcommands: eval, determinism, bench (WP-12), wasm

use anyhow::Result;
use clap::{Parser, Subcommand};

mod bench;
mod determinism;
mod eval;
mod wasm;

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
    /// Index a repo twice into fresh stores and assert byte-identical output.
    Determinism(determinism::DeterminismArgs),
    /// Time index + a few representative queries against a repo.
    Bench(bench::BenchArgs),
    /// Build the engine as a wasm32-wasip1 reactor module (needs a pinned wasi-sdk).
    Wasm(wasm::WasmArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Eval(args) => eval::run(args),
        Commands::Determinism(args) => determinism::run(args),
        Commands::Bench(args) => bench::run(args),
        Commands::Wasm(args) => wasm::run(args),
    }
}
