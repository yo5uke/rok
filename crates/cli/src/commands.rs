//! The commands. They follow the four principles of requirements chapter 5: all or nothing,
//! minimal change, show what changes, and (in the R package) prompt to restart.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context as _, bail};
use rok_core::dcf::is_package_name;
use rok_core::github;
use rok_core::install;
use rok_core::lockfile::{Lockfile, ManifestCopy, Snapshot, Source};
use rok_core::manifest::{self, DependencySource, DependencySpec, GitRef, Manifest};
use rok_core::ops::{self, Env, plural};
use rok_core::p3m::Index;
use rok_core::project::Project;
use rok_core::rdetect::RInstallation;
use rok_core::resolve::Origin;
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
            "{} package{} must be built from source:",
            sources.len(),
            plural(sources.len()),
        ));
        let minor = r.version.minor();
        for w in &sources {
            let why = match &w.origin {
                Origin::Snapshot(_) => format!("no binary for R {minor} on {}", env.platform),
                Origin::Repository { alias, .. } => {
                    format!("`{alias}` has no binary for R {minor} on {}", env.platform)
                }
                Origin::GitHub { owner, repo, .. } => format!("from GitHub {owner}/{repo}"),
            };
            ui.bullet(&format!("{} {} ({why})", w.name, w.version));
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
    ops::record_remotes(lock, &report.paths);
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
    latest: bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    if version.is_some() && packages.len() != 1 {
        bail!("`--version` applies to one package; add the packages one at a time.");
    }
    let constraint: rok_core::constraint::Constraint = match &version {
        Some(v) => v.parse()?,
        None => Default::default(),
    };
    let mut cran = Vec::new();
    let mut from_github = Vec::new();
    for p in packages {
        if p.contains('/') {
            let Some(spec) = github::parse_spec(p) else {
                bail!(
                    "`{p}` is not a GitHub repository; use the form owner/repo or owner/repo@ref."
                );
            };
            if version.is_some() {
                bail!(
                    "`--version` applies to CRAN packages; a GitHub package is fixed by its commit."
                );
            }
            if latest {
                bail!(
                    "`--latest` applies to CRAN packages; a GitHub package is fixed by its commit."
                );
            }
            from_github.push(spec);
            continue;
        }
        if !is_package_name(p) {
            bail!("`{p}` is not a valid R package name.");
        }
        if is_base(p) {
            bail!("{p} is part of R itself and does not need to be added.");
        }
        cran.push(p.clone());
    }

    let project = find_project(project_dir)?;
    let (mut doc, manifest) = project.read_manifest()?;
    let old_lock = project.read_lock()?;
    let env = Env::from_env()?;
    let date = &manifest.project.snapshot;

    // `--latest`: the packages come from the latest snapshot; the project's snapshot stays.
    let mut keep = ops::Keep::default();
    if latest {
        let (newest, dates) = env.p3m.resolve_snapshot(None)?;
        if dates.stale {
            ui.warn("Could not reach P3M; using the snapshot dates cached earlier.");
        }
        if newest == *date {
            ui.info(&format!(
                "The project's snapshot ({date}) is already the latest one."
            ));
        } else {
            keep.latest = cran.iter().cloned().collect();
            keep.newer = Some(newest);
        }
    }
    let lookup = keep.newer.as_ref().unwrap_or(date);
    let index = env.p3m.index(lookup)?;
    for p in &cran {
        if index.get(p).is_none() {
            let similar = index
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(p))
                .map(|e| e.name.clone());
            match similar {
                Some(s) => bail!("{p} is not in the {lookup} snapshot. Did you mean `{s}`?"),
                None if latest => {
                    bail!("{p} is not in the latest snapshot ({lookup}).")
                }
                None => bail!(
                    "{p} is not in the {date} snapshot.\nIf it was released after {date}, move the project's snapshot forward with `rok update`, or add only this package from the latest snapshot with `rok add --latest`."
                ),
            }
        }
    }

    // Build-time environment variables stay when a declaration is replaced.
    let env_of = |name: &str| {
        manifest
            .dependency(name)
            .map(|d| d.env.clone())
            .unwrap_or_default()
    };
    for p in &cran {
        let spec = DependencySpec {
            source: DependencySource::Cran {
                constraint: constraint.clone(),
            },
            env: env_of(p),
        };
        doc.set_dependency(p, &spec);
    }
    let gh = env.github();
    for (owner, repo, reference) in from_github {
        ui.step(&format!("Looking up {owner}/{repo} on GitHub"));
        let reference = match reference {
            None => GitRef::DefaultBranch,
            Some(r) => gh.classify(&owner, &repo, &r)?,
        };
        let (entry, _) = ops::read_github(&gh, &owner, &repo, &reference)?;
        let name = entry.name;
        if is_base(&name) {
            bail!("{owner}/{repo} is {name}, which is part of R itself.");
        }
        // `track = true` stays when the same repository's branch is added again.
        let track = matches!(
            manifest.dependency(&name).map(|d| &d.source),
            Some(DependencySource::GitHub { owner: o, repo: r, reference: re, track: true })
                if *o == owner && *r == repo && *re == reference
        );
        let spec = DependencySpec {
            source: DependencySource::GitHub {
                owner,
                repo,
                reference,
                track,
            },
            env: env_of(&name),
        };
        doc.set_dependency(&name, &spec);
    }
    let new_text = doc.to_string();
    let new_manifest = Manifest::parse(&new_text)?;
    let lock = apply(
        ui,
        &env,
        &project,
        &new_manifest,
        &new_text,
        old_lock,
        &keep,
        "add",
        start,
    )?;
    let mixed: Vec<String> = cran
        .iter()
        .filter_map(|n| lock.package(n))
        .filter_map(|p| {
            p.source
                .snapshot()
                .filter(|d| d != date)
                .map(|d| format!("{} ({d})", p.name))
        })
        .collect();
    if latest && !mixed.is_empty() {
        ui.info(&format!(
            "From the latest snapshot: {}; the project's snapshot stays {date}.",
            mixed.join(", ")
        ));
    }
    Ok(())
}

/// The declared package a command-line name refers to: `owner/repo` names a GitHub package.
fn declared_name(manifest: &Manifest, arg: &str) -> anyhow::Result<String> {
    if !arg.contains('/') {
        return Ok(arg.to_string());
    }
    let Some((owner, repo, None)) = github::parse_spec(arg) else {
        bail!("`{arg}` is not a package name or a GitHub repository (owner/repo).");
    };
    manifest
        .dependencies
        .iter()
        .find(|(_, spec)| {
            matches!(&spec.source, DependencySource::GitHub { owner: o, repo: r, .. }
                if o.eq_ignore_ascii_case(&owner) && r.eq_ignore_ascii_case(&repo))
        })
        .map(|(name, _)| name.clone())
        .ok_or_else(|| anyhow::anyhow!("No package from {owner}/{repo} is declared in rok.toml."))
}

// ---- remove ----

pub fn remove(ui: &Ui, project_dir: Option<&Path>, packages: &[String]) -> anyhow::Result<()> {
    let start = Instant::now();
    let project = find_project(project_dir)?;
    let (mut doc, manifest) = project.read_manifest()?;
    let old_lock = project.read_lock()?;
    let packages = packages
        .iter()
        .map(|p| declared_name(&manifest, p))
        .collect::<anyhow::Result<Vec<String>>>()?;
    for p in &packages {
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
    for p in &packages {
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
        &ops::Keep::default(),
        "remove",
        start,
    )
    .map(|_| ())
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
    keep: &ops::Keep,
    command: &str,
    start: Instant,
) -> anyhow::Result<Lockfile> {
    let r = project_r(ui, env, manifest, old_lock.as_ref())?;
    let lock_r = old_lock
        .as_ref()
        .filter(|l| l.r.minor() == r.version.minor())
        .map_or(&r.version, |l| &l.r)
        .clone();
    ui.step("Resolving dependencies");
    let mut lock = ops::resolve_lock_with(env, manifest, old_lock.as_ref(), &lock_r, keep)?;
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
    Ok(lock)
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

// ---- update ----

pub fn update(
    ui: &Ui,
    project_dir: Option<&Path>,
    packages: &[String],
    to: Option<String>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let project = find_project(project_dir)?;
    let (mut doc, manifest) = project.read_manifest()?;
    let old_lock = project
        .read_lock()?
        .ok_or_else(|| anyhow::anyhow!("rok.lock is missing; run `rok sync` first."))?;
    let packages = packages
        .iter()
        .map(|p| declared_name(&manifest, p))
        .collect::<anyhow::Result<Vec<String>>>()?;
    let env = Env::from_env()?;
    let r = project_r(ui, &env, &manifest, Some(&old_lock))?;
    let lock_r = if old_lock.r.minor() == r.version.minor() {
        old_lock.r.clone()
    } else {
        r.version.clone()
    };
    let (target, dates) = env.p3m.resolve_snapshot(to.as_deref())?;
    if dates.stale {
        ui.warn("Could not reach P3M; using the snapshot dates cached earlier.");
    }
    if let Some(t) = &to
        && *t != target
    {
        ui.info(&format!(
            "{t} is not a snapshot date; using the snapshot of {target}."
        ));
    }
    let current = manifest.project.snapshot.clone();
    let moving_back = packages.is_empty() && target < current;

    ui.step("Resolving dependencies");
    let (new_manifest, lock) = if packages.is_empty() {
        if moving_back {
            ui.warn(&format!("This moves the snapshot back from {current} to {target}; packages may be downgraded."));
        } else if target == current {
            ui.info(&format!("The snapshot is already {target}."));
        } else {
            ui.info(&format!("Moving the snapshot from {current} to {target}."));
        }
        doc.set_snapshot(&target);
        let m = Manifest::parse(&doc.to_string())?;
        let keep = ops::Keep {
            nothing: true,
            ..Default::default()
        };
        let lock = ops::resolve_lock_with(&env, &m, Some(&old_lock), &lock_r, &keep)?;
        (m, lock)
    } else {
        for p in &packages {
            if old_lock.package(p).is_none() {
                bail!("{p} is not in rok.lock.");
            }
        }
        let keep = ops::Keep {
            unlock: packages.iter().cloned().collect(),
            newer: Some(target.clone()),
            ..Default::default()
        };
        let lock = ops::resolve_lock_with(&env, &manifest, Some(&old_lock), &lock_r, &keep)?;
        (manifest.clone(), lock)
    };

    let changes = ops::diff(Some(&old_lock), &lock);
    if changes.is_empty() {
        ui.info("No package versions change.");
    } else {
        ui.changes(&changes);
    }
    // Declared constraints that keep a package below the newest version (requirements: shown
    // so that a held-back package is never a surprise).
    let newest = env.p3m.index(&target)?;
    for (name, spec) in &new_manifest.dependencies {
        if let manifest::DependencySource::Cran { constraint } = &spec.source
            && let (Some(locked), Some(entry)) = (lock.package(name), newest.get(name))
            && !constraint.matches(&entry.version)
            && locked.version != entry.version
        {
            ui.held(
                name,
                locked.version.as_str(),
                &format!("held back by {constraint}; {} is available", entry.version),
            );
        }
    }
    github_notices(ui, &env, &new_manifest, &lock, &newest, packages.is_empty());
    if !packages.is_empty() {
        let moved: Vec<&str> = changes
            .iter()
            .filter(|c| {
                lock.package(&c.name)
                    .is_some_and(|p| p.source.snapshot().is_some())
            })
            .map(|c| c.name.as_str())
            .collect();
        if !moved.is_empty() {
            ui.info(&format!(
                "{} now come{} from the {target} snapshot; the project's snapshot stays {current}.",
                moved.join(", "),
                if moved.len() == 1 { "s" } else { "" }
            ));
        }
    }
    if dry_run {
        ui.info("Dry run: nothing was changed.");
        ui.result(json!({ "command": "update", "dry_run": true, "snapshot": target, "changes": changes_json(&changes) }));
        return Ok(());
    }
    if !changes.is_empty() {
        let downgrades = changes
            .iter()
            .any(|c| matches!((&c.from, &c.to), (Some(f), Some(t)) if t < f));
        if !ui.confirm("Apply these changes?", !(moving_back || downgrades))? {
            bail!("Cancelled. Nothing was changed.");
        }
    }
    let mut lock = lock;
    let report = sync_library(ui, &env, &project, &new_manifest, &mut lock, &r)?;
    project.save(&doc.to_string(), &lock, true)?;
    summary(ui, &report, &lock, start);
    ui.result(report_json("update", &changes, &report));
    Ok(())
}

/// After an update: GitHub packages whose version is now on CRAN, and (when the whole project
/// was updated) kept GitHub packages whose branch has new commits.
fn github_notices(
    ui: &Ui,
    env: &Env,
    manifest: &Manifest,
    lock: &Lockfile,
    cran: &Index,
    whole: bool,
) {
    let gh = env.github();
    for p in &lock.packages {
        let Source::GitHub {
            owner,
            repo,
            reference,
            commit,
        } = &p.source
        else {
            continue;
        };
        if let Some(e) = cran.get(&p.name)
            && e.version >= p.version
        {
            ui.info(&format!(
                "{} {} is on CRAN; to switch from GitHub, run `rok add {}`.",
                p.name, e.version, p.name
            ));
        }
        let tracked = matches!(
            manifest.dependency(&p.name).map(|d| &d.source),
            Some(DependencySource::GitHub { track: true, .. })
        );
        let follows_branch = matches!(reference, GitRef::DefaultBranch | GitRef::Branch(_));
        if !whole || tracked || !follows_branch {
            continue;
        }
        match gh.resolve(owner, repo, reference) {
            Ok(head) if head != *commit => {
                let count = gh
                    .ahead_by(owner, repo, commit, &head)
                    .map(|n| format!("{n} new commit{}", plural(n as usize)))
                    .unwrap_or_else(|| "new commits".to_string());
                ui.info(&format!(
                    "{owner}/{repo} has {count} since {}; update {} with `rok update {owner}/{repo}`.",
                    &commit[..7],
                    p.name
                ));
            }
            Ok(_) => {}
            Err(e) => ui.warn(&format!(
                "Could not check {owner}/{repo} for new commits: {e}"
            )),
        }
    }
}

// ---- undo ----

pub fn undo(ui: &Ui, project_dir: Option<&Path>) -> anyhow::Result<()> {
    let start = Instant::now();
    let project = find_project(project_dir)?;
    let Some((manifest_text, Some(saved_lock))) = project.read_undo()? else {
        bail!("Nothing to undo.");
    };
    let manifest = Manifest::parse(&manifest_text)?;
    let current = project.read_lock()?;
    let env = Env::from_env()?;
    let r = project_r(ui, &env, &manifest, Some(&saved_lock))?;
    let changes = ops::diff(current.as_ref(), &saved_lock);
    let mut lock = saved_lock;
    let report = sync_library(ui, &env, &project, &manifest, &mut lock, &r)?;
    project.save(&manifest_text, &lock, false)?;
    project.clear_undo()?;
    ui.success("Undid the last change to rok.toml and rok.lock.");
    ui.changes(&changes);
    summary(ui, &report, &lock, start);
    ui.result(report_json("undo", &changes, &report));
    Ok(())
}

// ---- status ----

/// Prints the project's status. Returns whether there is a problem to fix (an error or a
/// warning), for `--check`.
pub fn status(ui: &Ui, project_dir: Option<&Path>, packages: bool) -> anyhow::Result<bool> {
    use rok_core::status::{self, Level};

    let project = find_project(project_dir)?;
    let (_, manifest) = project.read_manifest()?;
    let lock = project.read_lock()?;
    let env = Env::from_env()?;
    let installs = env.r_installations();
    let (minor, pinned) = ops::manifest_r(&manifest);
    let r = ops::select_r(&installs, &minor, pinned.or(lock.as_ref().map(|l| &l.r)));
    let library = project.library(&minor, &env.platform);
    let entries = status::read_library(&library, env.cache.root());
    let problems = status::check(&status::Inputs {
        manifest: &manifest,
        lock: lock.as_ref(),
        r,
        library: &entries,
    });
    let failing = problems.iter().any(|p| p.level != Level::Info);

    let r_label = r.map_or_else(
        || format!("R {minor} not installed"),
        |r| format!("R {}", r.version),
    );
    let n = lock.as_ref().map_or(0, |l| l.packages.len());
    let head = format!(
        "rok: {} ({r_label}, {n} package{}, snapshot {})",
        manifest.project.name,
        plural(n),
        manifest.project.snapshot
    );
    if failing {
        ui.line_plain(&head);
    } else {
        ui.success(&head);
    }
    for p in &problems {
        ui.level(p.level, &p.message);
        let limit = if packages { usize::MAX } else { 10 };
        for d in p.details.iter().take(limit) {
            ui.bullet(d);
        }
        if p.details.len() > limit {
            ui.bullet(&format!(
                "… and {} more (see `rok status --packages`)",
                p.details.len() - limit
            ));
        }
        if let Some(fix) = &p.fix {
            ui.step(&format!("→ {fix}"));
        }
    }
    if packages && let Some(l) = &lock {
        ui.line_plain("Packages:");
        for p in &l.packages {
            let declared = match manifest.dependency(&p.name).map(|d| &d.source) {
                Some(
                    DependencySource::Cran { constraint }
                    | DependencySource::Repository { constraint, .. },
                ) if !constraint.is_any() => format!(", declared {constraint}"),
                Some(_) => ", declared".to_string(),
                None => String::new(),
            };
            ui.bullet(&format!(
                "{} {} ({}{declared})",
                p.name,
                p.version,
                p.source.describe()
            ));
        }
    }
    ui.result(json!({
        "command": "status",
        "project": manifest.project.name,
        "r": r.map(|r| r.version.to_string()),
        "snapshot": manifest.project.snapshot,
        "ok": !failing,
        "problems": problems.iter().map(|p| json!({
            "level": p.level.as_str(),
            "code": p.code,
            "message": p.message,
            "details": p.details,
            "fix": p.fix,
        })).collect::<Vec<_>>(),
        "packages": lock.iter().flat_map(|l| l.packages.iter()).map(|p| {
            json!({
                "name": p.name,
                "version": p.version.to_string(),
                "source": ops::origin_label(&p.source),
                "snapshot": p.source.snapshot(),
                "declared": manifest.dependency(&p.name).is_some(),
                "installed": entries.get(&p.name).and_then(|i| i.version()).map(|v| v.to_string()),
            })
        }).collect::<Vec<_>>(),
    }));
    Ok(failing)
}

// ---- why and tree ----

fn lock_or_fail(project: &Project) -> anyhow::Result<Lockfile> {
    project
        .read_lock()?
        .ok_or_else(|| anyhow::anyhow!("rok.lock is missing; run `rok sync` first."))
}

pub fn why(ui: &Ui, project_dir: Option<&Path>, package: &str) -> anyhow::Result<()> {
    let project = find_project(project_dir)?;
    let (_, manifest) = project.read_manifest()?;
    let lock = lock_or_fail(&project)?;
    let declared: Vec<String> = manifest
        .dependencies
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    let Some(lines) = rok_core::graph::why(&lock, &declared, package) else {
        bail!("{package} is not in rok.lock.");
    };
    print_lines(ui, "why", &lines);
    Ok(())
}

pub fn tree(
    ui: &Ui,
    project_dir: Option<&Path>,
    package: Option<String>,
    depth: Option<usize>,
) -> anyhow::Result<()> {
    let project = find_project(project_dir)?;
    let (_, manifest) = project.read_manifest()?;
    let lock = lock_or_fail(&project)?;
    let roots: Vec<String> = match package {
        Some(p) if lock.package(&p).is_none() => bail!("{p} is not in rok.lock."),
        Some(p) => vec![p],
        None => manifest
            .dependencies
            .iter()
            .map(|(n, _)| n.clone())
            .collect(),
    };
    print_lines(ui, "tree", &rok_core::graph::tree(&lock, &roots, depth));
    Ok(())
}

/// Prints a drawing on stdout, or, with `--json`, its lines as JSON.
fn print_lines(ui: &Ui, command: &str, lines: &[String]) {
    if ui.json {
        ui.result(json!({ "command": command, "lines": lines }));
    } else {
        for l in lines {
            println!("{l}");
        }
    }
}
