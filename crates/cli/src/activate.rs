//! `rok activate`: run by `.rok/activate.R` each time R starts in a project (requirements
//! chapter 8). It decides whether the library matches rok.lock and R, syncs it when that is
//! light, asks before a sync that builds from source, and tells R which library to use.
//!
//! activate.R works with base R alone, so the output on stdout is `key=value` lines: always
//! `library=<path>`; for a question, exit status 2 with `choice=`, `question=` and one
//! `option=<value>\t<label>` line per answer (activate.R asks and runs again with
//! `--choice <value>`). Exit status 3 means strict mode: activate.R stops R.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use std::collections::BTreeSet;

use rok_core::install::{self, LinkReport};
use rok_core::lockfile::Lockfile;
use rok_core::manifest::{Manifest, NonInteractive, OnStartup};
use rok_core::ops::{self, Env, plural};
use rok_core::rdetect::{self, RInstallation, RKind};
use rok_core::scan::{self, Mode};

use crate::commands::{explain_missing_libraries, find_project, suggest_undeclared, summary};
use crate::ui::Ui;

/// The answer to the question asked before syncing at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SyncChoice {
    /// Sync everything, building from source where needed.
    All,
    /// Sync only the packages that need no build (and nothing that depends on one).
    Binaries,
    /// Do not sync now.
    None,
}

/// Exit status that makes activate.R stop R (`noninteractive = "error"`).
const STRICT: u8 = 3;
/// Exit status for a question activate.R must ask.
const QUESTION: u8 = 2;

pub fn activate(
    ui: &Ui,
    r_home: &Path,
    interactive: bool,
    choice: Option<SyncChoice>,
) -> anyhow::Result<ExitCode> {
    let start = Instant::now();
    let project = find_project(None)?;
    let (_, manifest) = project.read_manifest()?;
    let env = Env::from_env()?;
    let version = rdetect::r_home_version(r_home)
        .ok_or_else(|| anyhow::anyhow!("{} is not an R installation.", r_home.display()))?;
    let r = RInstallation {
        version,
        r_home: r_home.to_path_buf(),
        executable: r_home.join("bin").join("R"),
        kind: RKind::Path,
    };
    // A newer rok may have a newer hook; it takes effect at the next start.
    let _ = project.refresh_activate();
    let (minor, _) = ops::manifest_r(&manifest);
    let name = &manifest.project.name;
    let strict = !interactive && manifest.sync.noninteractive == NonInteractive::Error;

    // Packages built for another minor version of R cannot be used: an empty library keeps
    // them (and the user's own) out of reach.
    if r.version.minor() != minor {
        let isolated = project.rok_dir().join("library").join("isolated");
        std::fs::create_dir_all(&isolated)?;
        println!("library={}", isolated.display());
        explain_r_mismatch(ui, &env, &manifest, &r, interactive);
        return Ok(if strict {
            ExitCode::from(STRICT)
        } else {
            ExitCode::SUCCESS
        });
    }
    let library = project.library(&minor, &env.platform);
    println!("library={}", library.display());

    let old_lock = project.read_lock()?;
    if let Some(l) = &old_lock
        && interactive
        && l.r != r.version
    {
        ui.info(&format!(
            "Using R {}; rok.lock records R {}.",
            r.version, l.r
        ));
    }
    let current = old_lock
        .as_ref()
        .filter(|l| ops::lock_is_current(&manifest, l));

    // In sync? This is the path taken at almost every start, so it uses no network.
    if let Some(l) = current
        && let Some(paths) = ops::cached_library(&env, l, &manifest, &r)?
        && install::link_changes(&library, &paths, env.cache.root())? == LinkReport::default()
    {
        if interactive {
            let n = l.packages.len();
            let unmanaged = l
                .packages
                .iter()
                .filter(|p| p.source == rok_core::lockfile::Source::Unmanaged)
                .count();
            let unmanaged = if unmanaged > 0 {
                format!(", {unmanaged} unmanaged")
            } else {
                String::new()
            };
            ui.success(&format!(
                "rok: {name} (R {}, {n} package{}{unmanaged})",
                r.version,
                plural(n)
            ));
            suggest_new(ui, &project.root, &manifest, Some(l));
        }
        return Ok(ExitCode::SUCCESS);
    }

    let what = if current.is_none() {
        "rok.toml has changed since rok.lock was written"
    } else {
        "the library does not match rok.lock"
    };
    if !interactive {
        match manifest.sync.noninteractive {
            NonInteractive::Warn => {
                ui.warn(&format!(
                    "rok: {name}: {what}; R is not interactive, so it was not synced."
                ));
                ui.bullet("Run `rok::sync()` (or `rok sync` in a terminal) to sync.");
                return Ok(ExitCode::SUCCESS);
            }
            NonInteractive::Error => {
                ui.error(&format!("rok: {name}: {what}."));
                ui.bullet("Run `rok sync` before running R non-interactively.");
                return Ok(ExitCode::from(STRICT));
            }
            NonInteractive::Auto => {}
        }
    } else if manifest.sync.on_startup == OnStartup::Notify {
        ui.warn(&format!("rok: {name}: {what}."));
        ui.bullet("Run `rok::sync()` to sync.");
        return Ok(ExitCode::SUCCESS);
    }

    // rok.lock first, if rok.toml changed.
    let (mut lock, changes, relocked) = match current {
        Some(l) => (l.clone(), Vec::new(), false),
        None => {
            ui.step(&format!(
                "rok: {name}: rok.toml changed; resolving dependencies"
            ));
            let lock_r = old_lock
                .as_ref()
                .filter(|l| l.r.minor() == minor)
                .map_or(&r.version, |l| &l.r)
                .clone();
            match ops::resolve_lock(&env, &project.root, &manifest, old_lock.as_ref(), &lock_r) {
                Ok(lock) => {
                    let changes = ops::diff(old_lock.as_ref(), &lock);
                    (lock, changes, true)
                }
                Err(e) => {
                    ui.warn(&format!(
                        "rok: could not update rok.lock, so nothing was synced: {e}"
                    ));
                    ui.bullet("Run `rok::sync()` once the problem is fixed.");
                    return Ok(ExitCode::SUCCESS);
                }
            }
        }
    };

    // Without the network, sync what the cache has.
    let (plan, mut skipped) = match ops::plan_sync(&env, &lock, &manifest, &r) {
        Ok(plan) => (plan, Vec::new()),
        Err(e) if e.is_offline() => {
            ui.warn("rok: P3M cannot be reached; syncing only what the cache has.");
            ops::plan_sync_offline(&env, &lock, &manifest, &r)?
        }
        Err(e) => return Err(e.into()),
    };
    let sources: Vec<String> = plan.sources().iter().map(|w| w.name.clone()).collect();
    let answer = match choice {
        Some(c) => c,
        None if !interactive => SyncChoice::All,
        None if !sources.is_empty() => {
            ui.warn(&format!(
                "rok: {name}: {} package{} must be built from source: {}",
                sources.len(),
                plural(sources.len()),
                sources.join(", ")
            ));
            let missing = install::missing_build_tools(&r);
            if !missing.is_empty() {
                ui.bullet(&format!(
                    "Building needs tools that were not found: {}. On Debian or Ubuntu: sudo apt-get install -y build-essential",
                    missing.join(", ")
                ));
            }
            return ask(
                "How should rok sync the project?",
                &[
                    ("all", "Sync everything (build from source)"),
                    ("binaries", "Sync only what needs no build"),
                    ("none", "Do not sync now"),
                ],
            );
        }
        None if manifest.sync.on_startup == OnStartup::Ask => {
            ui.info(&format!("rok: {name}: {what}."));
            return ask(
                "Sync the project now?",
                &[("all", "Sync now"), ("none", "Not now")],
            );
        }
        None => SyncChoice::All,
    };
    let plan = match answer {
        SyncChoice::None => {
            ui.info(&format!(
                "rok: {name} was not synced. Run `rok::sync()` when you are ready."
            ));
            return Ok(ExitCode::SUCCESS);
        }
        SyncChoice::Binaries => {
            let excluded = sources
                .into_iter()
                .map(|n| (n, "must be built from source".to_string()))
                .collect();
            let (plan, more) = plan.without(excluded, &lock);
            skipped.extend(more);
            plan
        }
        SyncChoice::All => plan,
    };

    let before = lock.clone();
    let report = ops::execute_sync(&env, &plan, &library, &r, &|m| ui.step(m))?;
    ops::record_built_checksums(&mut lock, &report.built);
    ops::record_remotes(&mut lock, &report.paths);
    ops::record_rebuilt(&mut lock, &report.built);
    if relocked || lock != before {
        project.save_lock(&lock)?;
    }
    ui.changes(&changes);
    // Packages whose versions did not change but whose links were missing or out of date.
    let changed: std::collections::HashSet<&str> =
        changes.iter().map(|c| c.name.as_str()).collect();
    let relinked: Vec<&str> = report
        .link
        .linked
        .iter()
        .map(String::as_str)
        .filter(|n| !changed.contains(n))
        .collect();
    if !relinked.is_empty() {
        ui.info(&format!("Restored in the library: {}", relinked.join(", ")));
    }
    let unlinked: Vec<&str> = report
        .link
        .removed
        .iter()
        .map(String::as_str)
        .filter(|n| !changed.contains(n))
        .collect();
    if !unlinked.is_empty() {
        ui.info(&format!(
            "Removed from the library: {}",
            unlinked.join(", ")
        ));
    }
    if !skipped.is_empty() {
        ui.warn(&format!(
            "{} package{} not synced:",
            skipped.len(),
            if skipped.len() == 1 { " was" } else { "s were" }
        ));
        for (n, why) in &skipped {
            ui.bullet(&format!("{n} ({why})"));
        }
    }
    let missing = ops::missing_libraries(&env.platform, &report.paths, &r);
    if !missing.is_empty() {
        let advice = ops::library_advice(&env, &lock, &missing, true);
        explain_missing_libraries(ui, &missing, &advice);
    }
    if interactive {
        suggest_new(ui, &project.root, &manifest, Some(&lock));
    }
    if skipped.is_empty() {
        summary(ui, &report, &lock, start);
    } else {
        let total = lock.packages.len();
        ui.warn(&format!(
            "rok: {name}: {} of {total} package{} in sync; run `rok::sync()` to sync the rest.",
            total - skipped.len(),
            plural(total)
        ));
    }
    Ok(ExitCode::SUCCESS)
}

/// Suggests declaring the packages the code newly uses (requirements chapter 8): only changed
/// files are read, and each package is suggested once.
fn suggest_new(ui: &Ui, root: &Path, manifest: &Manifest, lock: Option<&Lockfile>) {
    let Ok((_, findings)) = ops::scan_findings(root, manifest, lock, Mode::Quick) else {
        return;
    };
    let names: BTreeSet<String> = findings.undeclared.iter().map(|(n, _)| n.clone()).collect();
    let Ok(new) = scan::new_since_last_time(root, &names) else {
        return;
    };
    let shown = ops::ScanFindings {
        undeclared: findings
            .undeclared
            .into_iter()
            .filter(|(n, _)| new.contains(n))
            .collect(),
        unused: Vec::new(),
    };
    suggest_undeclared(ui, &shown, "rok::add");
}

/// Hands a question to activate.R.
fn ask(question: &str, options: &[(&str, &str)]) -> anyhow::Result<ExitCode> {
    println!("choice=sync");
    println!("question={question}");
    for (value, label) in options {
        println!("option={value}\t{label}");
    }
    Ok(ExitCode::from(QUESTION))
}

/// The message for a session whose R has another minor version than the project's
/// (requirements chapter 8).
fn explain_r_mismatch(
    ui: &Ui,
    env: &Env,
    manifest: &Manifest,
    r: &RInstallation,
    interactive: bool,
) {
    let (minor, pinned) = ops::manifest_r(manifest);
    let running = r.version.minor();
    ui.warn(&format!(
        "This project requires R {minor}, but the current R is {}.",
        r.version
    ));
    if !interactive {
        ui.bullet("Packages were not synced; the project library is not used.");
        return;
    }
    ui.info(&format!(
        "Packages built for R {minor} are not compatible with R {running}, so sync was skipped."
    ));
    ui.info("To fix this, do one of the following:");
    let installs = env.r_installations();
    match ops::select_r(&installs, &minor, pinned) {
        Some(i) => ui.bullet(&format!(
            "Switch your IDE's R to {} (installed at {})",
            i.version,
            i.r_home.display()
        )),
        None => ui.bullet(&format!(
            "Run `rok r install` in a terminal to install R {minor}"
        )),
    }
    ui.bullet(&format!(
        "Run `rok::pin_r(\"{running}\")` to migrate this project to R {running}"
    ));
}
