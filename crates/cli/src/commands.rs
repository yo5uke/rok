//! The commands. They follow the four principles of requirements chapter 5: all or nothing,
//! minimal change, show what changes, and (in the R package) prompt to restart.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, bail};
use rok_core::dcf::is_package_name;
use rok_core::install;
use rok_core::lockfile::{Lockfile, ManifestCopy, Snapshot};
use rok_core::manifest::{self, DependencySpec, Manifest};
use rok_core::ops::{self, Env, plural};
use rok_core::project::Project;
use rok_core::rdetect::RInstallation;
use rok_core::rpkgs::is_base;
use rok_core::version::Version;
use serde_json::json;

use crate::ui::{Ui, changes_json};

/// The project for `--project`, or the one containing the current directory.
fn find_project(dir: Option<&Path>) -> anyhow::Result<Project> {
    let start = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    Project::find(&start).ok_or_else(|| {
        anyhow::anyhow!(
            "No rok.toml found in {} or its parent directories.\nRun `rok init` to create a project.",
            start.display()
        )
    })
}

/// The installed R to use for a project.
fn project_r(
    ui: &Ui,
    env: &Env,
    manifest: &Manifest,
    lock: Option<&Lockfile>,
) -> anyhow::Result<RInstallation> {
    let (minor, pinned) = ops::manifest_r(manifest);
    let prefer = pinned.or(lock.map(|l| &l.r));
    let installs = env.r_installations();
    let Some(r) = ops::select_r(&installs, &minor, prefer).cloned() else {
        let mut found: Vec<String> = installs.iter().map(|i| i.version.to_string()).collect();
        found.dedup();
        let found = if found.is_empty() {
            "none".to_string()
        } else {
            found.join(", ")
        };
        bail!(
            "This project needs R {minor}, which is not installed (found: {found}).\nInstall R {minor}, or change the project's R version in rok.toml."
        );
    };
    if let Some(l) = lock
        && l.r.minor() == minor
        && l.r != r.version
    {
        ui.info(&format!(
            "Using R {}; rok.lock records R {}.",
            r.version, l.r
        ));
    }
    Ok(r)
}

/// Makes the project library match `lock`, asking first if packages must be built from
/// source. Records SHA-256 values learned while building in `lock`.
fn sync_library(
    ui: &Ui,
    env: &Env,
    project: &Project,
    manifest: &Manifest,
    lock: &mut Lockfile,
    r: &RInstallation,
) -> anyhow::Result<ops::SyncReport> {
    let plan = ops::plan_sync(env, lock, manifest, r)?;
    let sources = plan.sources();
    if !sources.is_empty() {
        ui.warn(&format!(
            "{} package{} must be built from source (no binary for R {} on {}):",
            sources.len(),
            plural(sources.len()),
            r.version.minor(),
            env.platform
        ));
        for w in &sources {
            ui.bullet(&format!("{} {}", w.name, w.version));
        }
        let missing = install::missing_build_tools(r);
        if !missing.is_empty() {
            ui.warn(&format!(
                "Building needs tools that were not found: {}.",
                missing.join(", ")
            ));
            ui.bullet("On Debian or Ubuntu: sudo apt-get install -y build-essential");
        }
        if !ui.confirm(
            &format!(
                "Build {} package{} from source?",
                sources.len(),
                plural(sources.len())
            ),
            true,
        )? {
            bail!("Cancelled. Nothing was changed.");
        }
    }
    let library = project.library(&r.version.minor(), &env.platform);
    let report = ops::execute_sync(env, &plan, &library, r, &|m| ui.step(m))?;
    ops::record_built_checksums(lock, &report.built);
    Ok(report)
}

fn summary(ui: &Ui, report: &ops::SyncReport, lock: &Lockfile, start: Instant) {
    let n = lock.packages.len();
    let mut detail = Vec::new();
    if report.downloaded > 0 {
        detail.push(format!("{} downloaded", report.downloaded));
    }
    if !report.built.is_empty() {
        detail.push(format!("{} built from source", report.built.len()));
    }
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(" ({})", detail.join(", "))
    };
    ui.success(&format!(
        "Library is in sync with rok.lock: {n} package{}{detail} in {:.1}s",
        plural(n),
        start.elapsed().as_secs_f64()
    ));
}

fn report_json(
    command: &str,
    changes: &[ops::Change],
    report: &ops::SyncReport,
) -> serde_json::Value {
    json!({
        "command": command,
        "changes": changes_json(changes),
        "downloaded": report.downloaded,
        "built": report.built.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
        "linked": report.link.linked,
        "unlinked": report.link.removed,
    })
}

// ---- init ----

pub fn init(
    ui: &Ui,
    path: Option<PathBuf>,
    r: Option<String>,
    name: Option<String>,
) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let dir = match path {
        Some(p) if p.is_absolute() => p,
        Some(p) => cwd.join(p),
        None => cwd,
    };
    if dir.join(manifest::FILE_NAME).exists() {
        bail!("{} is already a rok project.", dir.display());
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let dir = dir.canonicalize()?;
    if std::env::home_dir()
        .and_then(|h| h.canonicalize().ok())
        .as_deref()
        == Some(dir.as_path())
        && !ui.confirm(
            "This is your home directory. Create a rok project here?",
            false,
        )?
    {
        bail!("Cancelled. Nothing was changed.");
    }
    if let Some(parent) = dir.parent().and_then(Project::find)
        && !ui.confirm(
            &format!(
                "{} is inside the rok project at {}. Create a nested project?",
                dir.display(),
                parent.root.display()
            ),
            false,
        )?
    {
        bail!("Cancelled. Nothing was changed.");
    }

    let env = Env::from_env()?;
    let installs = env.r_installations();
    let r = match r.as_deref() {
        None => installs
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version).then(b.kind.cmp(&a.kind)))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No R installation was found.\nInstall R, then run `rok init` again."
                )
            })?,
        Some("latest") => bail!(
            "`--r latest` needs R installation support, which is not available yet; give a version such as 4.6."
        ),
        Some(v) => {
            let wanted: Version = v.parse().map_err(|_| {
                anyhow::anyhow!("`{v}` is not an R version; use the form 4.6 or 4.6.1.")
            })?;
            let exact = (wanted.parts().len() == 3).then_some(&wanted);
            ops::select_r(&installs, &wanted.minor(), exact)
                .filter(|i| exact.is_none_or(|e| e == &i.version))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("R {wanted} is not installed."))?
        }
    };

    let (date, dates) = env.p3m.resolve_snapshot(None)?;
    if dates.stale {
        ui.warn("Could not reach P3M; using the snapshot dates cached earlier.");
    }
    let name = name.unwrap_or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into())
    });
    let minor: Version = r.version.minor().parse()?;
    let manifest_text = manifest::new_manifest_text(&name, &minor, &date);
    let lock = Lockfile {
        generated_by: format!("rok {}", env!("CARGO_PKG_VERSION")),
        r: r.version.clone(),
        snapshot: Snapshot {
            date: date.clone(),
            repository: env.p3m.cran_base(),
        },
        manifest: ManifestCopy::default(),
        packages: Vec::new(),
    };
    let project = Project::new(&dir);
    project.save(&manifest_text, &lock, false)?;
    let rprofile_changed = project.install_startup_hook()?;
    std::fs::create_dir_all(project.library(&minor.to_string(), &env.platform))?;

    ui.success(&format!("Created project `{name}` in {}", dir.display()));
    ui.bullet(&format!(
        "R {minor} (using R {} at {})",
        r.version,
        r.r_home.display()
    ));
    ui.bullet(&format!("Snapshot {date}"));
    if rprofile_changed {
        ui.info("Added `source(\".rok/activate.R\")` to the top of your existing .Rprofile.");
    }
    ui.info("Add packages with `rok add <package>`.");
    ui.result(json!({
        "command": "init",
        "root": dir,
        "name": name,
        "r": r.version.to_string(),
        "snapshot": date,
    }));
    Ok(())
}

// ---- add ----

pub fn add(
    ui: &Ui,
    project_dir: Option<&Path>,
    packages: &[String],
    version: Option<String>,
) -> anyhow::Result<()> {
    let start = Instant::now();
    if version.is_some() && packages.len() != 1 {
        bail!("`--version` applies to one package; add the packages one at a time.");
    }
    let constraint: rok_core::constraint::Constraint = match &version {
        Some(v) => v.parse()?,
        None => Default::default(),
    };
    for p in packages {
        if p.contains('/') {
            bail!("{p}: GitHub packages are not supported yet.");
        }
        if !is_package_name(p) {
            bail!("`{p}` is not a valid R package name.");
        }
        if is_base(p) {
            bail!("{p} is part of R itself and does not need to be added.");
        }
    }

    let project = find_project(project_dir)?;
    let (mut doc, manifest) = project.read_manifest()?;
    let old_lock = project.read_lock()?;
    let env = Env::from_env()?;
    let date = &manifest.project.snapshot;
    let index = env.p3m.index(date)?;
    for p in packages {
        if index.get(p).is_none() {
            let similar = index
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(p))
                .map(|e| e.name.clone());
            match similar {
                Some(s) => bail!("{p} is not in the {date} snapshot. Did you mean `{s}`?"),
                None => bail!(
                    "{p} is not in the {date} snapshot.\nIf it was released after {date}, move the project's snapshot forward with `rok update`, or add only this package from the latest snapshot with `rok add --latest`."
                ),
            }
        }
    }

    for p in packages {
        doc.set_dependency(p, &DependencySpec::cran(constraint.clone()));
    }
    let new_text = doc.to_string();
    let new_manifest = Manifest::parse(&new_text)?;
    apply(
        ui,
        &env,
        &project,
        &new_manifest,
        &new_text,
        old_lock,
        "add",
        start,
    )
}

// ---- remove ----

pub fn remove(ui: &Ui, project_dir: Option<&Path>, packages: &[String]) -> anyhow::Result<()> {
    let start = Instant::now();
    let project = find_project(project_dir)?;
    let (mut doc, manifest) = project.read_manifest()?;
    let old_lock = project.read_lock()?;
    for p in packages {
        if manifest.dependency(p).is_some() {
            continue;
        }
        let users: Vec<&str> = old_lock
            .iter()
            .flat_map(|l| l.packages.iter())
            .filter(|pkg| pkg.dependencies.iter().any(|d| d == p))
            .map(|pkg| pkg.name.as_str())
            .collect();
        if users.is_empty() {
            bail!("{p} is not declared in rok.toml.");
        }
        bail!(
            "{p} is not declared in rok.toml; it is installed because {} need{} it.",
            users.join(", "),
            if users.len() == 1 { "s" } else { "" }
        );
    }
    for p in packages {
        doc.remove_dependency(p);
    }
    let new_text = doc.to_string();
    let new_manifest = Manifest::parse(&new_text)?;
    let env = Env::from_env()?;
    apply(
        ui,
        &env,
        &project,
        &new_manifest,
        &new_text,
        old_lock,
        "remove",
        start,
    )
}

/// Resolves the new manifest, syncs the library, and only then writes rok.toml and rok.lock
/// (keeping the previous ones for undo). If anything fails, the files stay as they were.
#[allow(clippy::too_many_arguments)]
fn apply(
    ui: &Ui,
    env: &Env,
    project: &Project,
    manifest: &Manifest,
    manifest_text: &str,
    old_lock: Option<Lockfile>,
    command: &str,
    start: Instant,
) -> anyhow::Result<()> {
    let r = project_r(ui, env, manifest, old_lock.as_ref())?;
    let lock_r = old_lock
        .as_ref()
        .filter(|l| l.r.minor() == r.version.minor())
        .map_or(&r.version, |l| &l.r)
        .clone();
    ui.step("Resolving dependencies");
    let mut lock = ops::resolve_lock(env, manifest, old_lock.as_ref(), &lock_r)?;
    let changes = ops::diff(old_lock.as_ref(), &lock);
    let report = sync_library(ui, env, project, manifest, &mut lock, &r)?;
    project.save(manifest_text, &lock, true)?;
    if changes.is_empty() {
        ui.info("No package versions changed.");
    } else {
        ui.changes(&changes);
    }
    summary(ui, &report, &lock, start);
    ui.result(report_json(command, &changes, &report));
    Ok(())
}

// ---- sync ----

pub fn sync(ui: &Ui, project_dir: Option<&Path>, locked: bool) -> anyhow::Result<()> {
    let start = Instant::now();
    let project = find_project(project_dir)?;
    let (_, manifest) = project.read_manifest()?;
    let old_lock = project.read_lock()?;
    let env = Env::from_env()?;
    let r = project_r(ui, &env, &manifest, old_lock.as_ref())?;
    let current = old_lock
        .as_ref()
        .filter(|l| ops::lock_is_current(&manifest, l));
    let (mut lock, changes, relocked) = match current {
        Some(l) => (l.clone(), Vec::new(), false),
        None if locked => bail!(
            "rok.lock is out of date with rok.toml, and `--locked` was given.\nRun `rok sync` without `--locked` to update rok.lock."
        ),
        None => {
            ui.step("rok.toml changed; resolving dependencies");
            let lock_r = old_lock
                .as_ref()
                .filter(|l| l.r.minor() == r.version.minor())
                .map_or(&r.version, |l| &l.r)
                .clone();
            let lock = ops::resolve_lock(&env, &manifest, old_lock.as_ref(), &lock_r)?;
            let changes = ops::diff(old_lock.as_ref(), &lock);
            (lock, changes, true)
        }
    };
    let before = lock.clone();
    let report = sync_library(ui, &env, &project, &manifest, &mut lock, &r)?;
    if relocked || lock != before {
        project.save_lock(&lock)?;
    }
    ui.changes(&changes);
    if !report.link.removed.is_empty() {
        ui.info(&format!(
            "Removed from the library: {}",
            report.link.removed.join(", ")
        ));
    }
    summary(ui, &report, &lock, start);
    ui.result(report_json("sync", &changes, &report));
    Ok(())
}
