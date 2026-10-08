//! Command-line entry point of rok.

mod commands;
mod rcmd;
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
    let ui = ui::Ui::new(cli.yes, cli.json, cli.confirmed);
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
        Command::Run { r, args } => match rcmd::run(&ui, project, r, &args) {
            Ok(code) => return code,
            Err(e) => Err(e),
        },
        Command::R { command } => match command {
            RCommand::List { all } => rcmd::list(&ui, project, all),
            RCommand::Install { version, system } => rcmd::install(&ui, project, version, system),
            RCommand::Uninstall { version } => rcmd::uninstall(&ui, &version),
            RCommand::Pin { version, strategy } => rcmd::pin(&ui, project, &version, strategy),
        },
        Command::Status { packages, check } => match commands::status(&ui, project, packages) {
            Ok(true) if check => return ExitCode::FAILURE,
            other => other.map(|_| ()),
        },
        Command::Why { package } => commands::why(&ui, project, &package),
        Command::Tree { package, depth } => commands::tree(&ui, project, package, depth),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        // A program asked rok to do something that needs an answer: hand the question back.
        Err(e) if e.downcast_ref::<ui::NeedsAnswer>().is_some() => {
            let needs = e.downcast_ref::<ui::NeedsAnswer>().expect("checked");
            println!("{}", needs.to_json());
            ExitCode::from(2)
        }
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
