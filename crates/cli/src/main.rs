//! Command-line entry point of rok.

mod commands;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// A fast package and project manager for R.
#[derive(Parser)]
#[command(name = "rok", version, arg_required_else_help = true)]
struct Cli {
    /// Use the project that contains this directory instead of the current one.
    #[arg(long, global = true, value_name = "DIR")]
    project: Option<PathBuf>,
    /// Answer yes to every question.
    #[arg(long, short, global = true)]
    yes: bool,
    /// Also print the result as JSON on stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a project: rok.toml, rok.lock and the startup hook.
    Init {
        /// The project directory (created if needed). Defaults to the current directory.
        path: Option<PathBuf>,
        /// The R version, such as 4.6 (default: the newest installed R).
        #[arg(long, value_name = "VERSION")]
        r: Option<String>,
        /// The project name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Add packages to rok.toml, then update rok.lock and the library.
    Add {
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
        /// A version constraint, such as "< 0.13" or ">= 1.0, < 2".
        #[arg(long, value_name = "CONSTRAINT")]
        version: Option<String>,
    },
    /// Remove packages from rok.toml, then update rok.lock and the library.
    Remove {
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
    },
    /// Make the library match rok.lock (updating rok.lock first if rok.toml changed).
    Sync {
        /// Fail instead of updating rok.lock when it is out of date.
        #[arg(long)]
        locked: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = ui::Ui::new(cli.yes, cli.json);
    let project = cli.project.as_deref();
    let result = match cli.command {
        Command::Init { path, r, name } => commands::init(&ui, path, r, name),
        Command::Add { packages, version } => commands::add(&ui, project, &packages, version),
        Command::Remove { packages } => commands::remove(&ui, project, &packages),
        Command::Sync { locked } => commands::sync(&ui, project, locked),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let mut msg = e.to_string();
            for cause in e.chain().skip(1) {
                msg.push_str(&format!("\n{cause}"));
            }
            ui.error(&msg);
            ExitCode::FAILURE
        }
    }
}
