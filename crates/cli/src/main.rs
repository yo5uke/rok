//! Command-line entry point of rok.

use clap::Parser;

/// A fast package and project manager for R.
#[derive(Parser)]
#[command(name = "rok", version, arg_required_else_help = true)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
