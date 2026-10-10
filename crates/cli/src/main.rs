//! Command-line entry point of rok.

mod activate;
mod commands;
mod import;
mod progress;
mod rcmd;
mod selfcmd;
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
    /// Answer yes to the question with this id (used by the R package after asking).
    #[arg(long, global = true, value_name = "ID", hide = true)]
    confirmed: Vec<String>,
    /// Write messages, progress, the process id and the exit status to this file (used by the
    /// R package, which runs rok in the background and draws the progress itself).
    #[arg(long, global = true, value_name = "FILE", hide = true)]
    events: Option<PathBuf>,
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
        /// CRAN package names, or GitHub repositories as owner/repo or owner/repo@ref.
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
        /// A version constraint, such as "< 0.13" or ">= 1.0, < 2".
        #[arg(long, value_name = "CONSTRAINT")]
        version: Option<String>,
        /// Take these packages from the latest snapshot, keeping the project's snapshot.
        #[arg(long)]
        latest: bool,
    },
    /// Remove packages from rok.toml, then update rok.lock and the library.
    Remove {
        /// Package names (or owner/repo for GitHub packages).
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
    },
    /// Make the library match rok.lock (updating rok.lock first if rok.toml changed).
    Sync {
        /// Fail instead of updating rok.lock when it is out of date.
        #[arg(long)]
        locked: bool,
    },
    /// Move the snapshot forward and resolve again, or update only the given packages.
    Update {
        /// Update only these packages (and what they need), keeping the project's snapshot.
        /// GitHub packages can be given as owner/repo.
        #[arg(value_name = "PACKAGE")]
        packages: Vec<String>,
        /// The snapshot date to move to (default: the latest published one).
        #[arg(long, value_name = "DATE")]
        to: Option<String>,
        /// Show what would change without changing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Undo the last add, remove, update or `r pin`.
    Undo,
    /// Run an R script with the project's R and library (after syncing them).
    Run {
        /// The R version to use instead of the project's, such as 4.5 or 4.5.1.
        #[arg(long, value_name = "VERSION")]
        r: Option<String>,
        /// The script, then arguments passed to it.
        #[arg(
            required = true,
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "FILE"
        )]
        args: Vec<String>,
    },
    /// Check the project when R starts, and sync it if needed (run by .rok/activate.R).
    #[command(hide = true)]
    Activate {
        /// R_HOME of the R that is starting.
        #[arg(long, value_name = "DIR")]
        r_home: PathBuf,
        /// Whether the R session is interactive.
        #[arg(long)]
        interactive: bool,
        /// The answer to the question about syncing.
        #[arg(long, value_enum)]
        choice: Option<activate::SyncChoice>,
    },
    /// Migrate a project from another tool, keeping its package versions.
    Import {
        #[command(subcommand)]
        from: ImportCommand,
    },
    /// Write another tool's lockfile from rok.lock.
    Export {
        #[command(subcommand)]
        to: ExportCommand,
    },
    /// Manage R itself: list, install, uninstall, and the project's R version.
    R {
        #[command(subcommand)]
        command: RCommand,
    },
    /// Show what is out of sync, without using the network.
    Status {
        /// List every package.
        #[arg(long)]
        packages: bool,
        /// Exit with status 1 if there is something to fix.
        #[arg(long)]
        check: bool,
    },
    /// Show why a package is installed.
    Why {
        #[arg(value_name = "PACKAGE")]
        package: String,
    },
    /// Show the dependency tree.
    Tree {
        /// Show only this package's dependencies.
        #[arg(value_name = "PACKAGE")]
        package: Option<String>,
        /// How many levels to show.
        #[arg(long, value_name = "N")]
        depth: Option<usize>,
    },
    /// Manage rok itself.
    #[command(name = "self")]
    SelfCmd {
        #[command(subcommand)]
        command: SelfCommand,
    },
}

#[derive(Subcommand)]
enum SelfCommand {
    /// Update rok (the binary and the copies of its R package) to the newest release.
    Update {
        /// Also consider pre-releases.
        #[arg(long)]
        prerelease: bool,
    },
}

#[derive(Subcommand)]
enum ImportCommand {
    /// Create rok.toml and rok.lock from renv.lock, with the same versions.
    Renv {
        /// The renv.lock to read (default: the one in the project directory).
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// What to do with packages from sources rok cannot install yet (asked if not given).
        #[arg(long, value_enum)]
        unsupported: Option<import::Unsupported>,
    },
}

#[derive(Subcommand)]
enum ExportCommand {
    /// Write renv.lock (without Hash), with P3M's dated repositories.
    Renv {
        /// Where to write it (default: renv.lock in the project).
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum RCommand {
    /// List installed R versions and the ones available to install.
    List {
        /// List every available version, not only the newest patch of each minor version.
        #[arg(long)]
        all: bool,
    },
    /// Install R without administrator rights (default: the version the project needs, or the
    /// newest release outside a project).
    Install {
        /// A version such as 4.6, 4.6.1 or latest.
        #[arg(value_name = "VERSION")]
        version: Option<String>,
        /// Show the commands that install R for all users in /opt/R instead (they need sudo).
        #[arg(long)]
        system: bool,
    },
    /// Remove an R version that rok installed.
    Uninstall {
        #[arg(value_name = "VERSION")]
        version: String,
    },
    /// Move the project to another R version (resolving its packages again).
    Pin {
        /// A version such as 4.6, 4.6.1 or latest.
        #[arg(value_name = "VERSION")]
        version: String,
        /// What to do with packages that have no binary for the new R (asked if not given).
        #[arg(long, value_enum)]
        strategy: Option<rcmd::Strategy>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let events = cli.events.is_some();
    let ui = match ui::Ui::new(cli.yes, cli.json, cli.confirmed, cli.events.as_deref()) {
        Ok(ui) => ui,
        Err(e) => {
            eprintln!("✖ Could not write to the events file: {e}");
            return ExitCode::FAILURE;
        }
    };
    if events {
        ui.event("pid", &std::process::id().to_string());
    }
    let project = cli.project.as_deref();
    let result = match cli.command {
        Command::Init { path, r, name } => commands::init(&ui, path, r, name),
        Command::Add {
            packages,
            version,
            latest,
        } => commands::add(&ui, project, &packages, version, latest),
        Command::Remove { packages } => commands::remove(&ui, project, &packages),
        Command::Sync { locked } => commands::sync(&ui, project, locked),
        Command::Update {
            packages,
            to,
            dry_run,
        } => commands::update(&ui, project, &packages, to, dry_run),
        Command::Undo => commands::undo(&ui, project),
        Command::Activate {
            r_home,
            interactive,
            choice,
        } => match activate::activate(&ui, &r_home, interactive, choice) {
            Ok(code) => return code,
            Err(e) => Err(e),
        },
        Command::Run { r, args } => match rcmd::run(&ui, project, r, &args) {
            Ok(code) => return code,
            Err(e) => Err(e),
        },
        Command::Import {
            from: ImportCommand::Renv { file, unsupported },
        } => import::import_renv(&ui, project, file, unsupported),
        Command::Export {
            to: ExportCommand::Renv { output },
        } => import::export_renv(&ui, project, output),
        Command::R { command } => match command {
            RCommand::List { all } => rcmd::list(&ui, project, all),
            RCommand::Install { version, system } => rcmd::install(&ui, project, version, system),
            RCommand::Uninstall { version } => rcmd::uninstall(&ui, &version),
            RCommand::Pin { version, strategy } => rcmd::pin(&ui, project, &version, strategy),
        },
        Command::Status { packages, check } => match commands::status(&ui, project, packages) {
            Ok(true) if check => Err(StatusCheckFailed.into()),
            other => other.map(|_| ()),
        },
        Command::Why { package } => commands::why(&ui, project, &package),
        Command::Tree { package, depth } => commands::tree(&ui, project, package, depth),
        Command::SelfCmd {
            command: SelfCommand::Update { prerelease },
        } => selfcmd::update(&ui, prerelease),
    };
    let code = match result {
        Ok(()) => 0,
        // A program asked rok to do something that needs an answer: hand the question back.
        Err(e) if e.downcast_ref::<ui::NeedsAnswer>().is_some() => {
            let needs = e.downcast_ref::<ui::NeedsAnswer>().expect("checked");
            println!("{}", needs.to_json());
            2
        }
        // `status --check` found problems: they were shown already.
        Err(e) if e.is::<StatusCheckFailed>() => 1,
        Err(e) => {
            let mut msg = e.to_string();
            for cause in e.chain().skip(1) {
                // Messages such as "<path>: <cause>" already show their cause.
                let cause = cause.to_string();
                if !msg.contains(&cause) {
                    msg.push_str(&format!("\n{cause}"));
                }
            }
            ui.error(&msg);
            1
        }
    };
    if events {
        ui.event("exit", &code.to_string());
    }
    ExitCode::from(code)
}

/// `rok status --check` found something to fix.
#[derive(Debug)]
struct StatusCheckFailed;

impl std::fmt::Display for StatusCheckFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The project needs attention.")
    }
}

impl std::error::Error for StatusCheckFailed {}
