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
use rok_core::scan::Mode;
use rok_core::version::Version;
use serde_json::json;

use crate::ui::{Ui, changes_json};

/// The project for `--project`, or the one containing the current directory.
pub(crate) fn find_project(dir: Option<&Path>) -> anyhow::Result<Project> {
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

/// The installed R to use for a project. If none has the project's minor version, offers to
/// install the version the project needs (requirements chapter 6).
pub(crate) fn project_r(
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
        ui.warn(&format!(
            "This project needs R {minor}, which is not installed (found: {found})."
        ));
        let (version, why) = crate::rcmd::needed_version(ui, env, manifest, lock)?;
        if !ui.confirm("install-r", &format!("Install R {version} ({why})?"), true)? {
            bail!(
                "Install R {minor} with `rok r install`, or move the project to an installed R with `rok r pin`."
            );
        }
        return crate::rcmd::install_r(ui, env, &version, &why);
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
pub(crate) fn sync_library(
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
        let rebuilds = plan.rebuilds();
        for w in &sources {
            let why = match &w.origin {
                Origin::Repository { alias, .. } if rebuilds.contains(w) => {
                    let remote = w.remote.as_ref().expect("rebuilds have a Git origin");
                    format!(
                        "`{alias}` no longer has it; rebuilt from {}@{}",
                        remote.url,
                        &remote.sha[..remote.sha.len().min(7)]
                    )
                }
                Origin::Snapshot(_) => format!("no binary for R {minor} on {}", env.platform),
                Origin::Repository { alias, .. } => {
                    format!("`{alias}` has no binary for R {minor} on {}", env.platform)
                }
                Origin::GitHub { owner, repo, .. } => format!("from GitHub {owner}/{repo}"),
                Origin::Unmanaged => "unmanaged".to_string(),
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
            "build-from-source",
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
    ops::record_rebuilt(lock, &report.built);
    let rebuilt: Vec<String> = report
        .built
        .iter()
        .filter(|b| b.git)
        .map(|b| b.name.clone())
        .collect();
    if !rebuilt.is_empty() {
        ui.warn(&format!(
            "Rebuilt from Git: {}. The result may differ from the release the repository distributed.",
            rebuilt.join(", ")
        ));
    }
    let missing = ops::missing_libraries(&env.platform, &report.paths, r);
    if !missing.is_empty() {
        let advice = ops::library_advice(env, lock, &missing, true);
        explain_missing_libraries(ui, &missing, &advice);
    }
    Ok(report)
}

/// Shows packages that cannot load until system libraries are installed, and the apt command.
pub(crate) fn explain_missing_libraries(
    ui: &Ui,
    missing: &std::collections::BTreeMap<String, Vec<String>>,
    advice: &rok_core::syslibs::AptAdvice,
) {
    ui.warn(&format!(
        "{} package{} cannot be loaded until these system libraries are installed:",
        missing.len(),
        plural(missing.len())
    ));
    for (name, libs) in missing {
        ui.bullet(&format!("{name}: {}", libs.join(", ")));
    }
    if let Some(cmd) = advice.command() {
        ui.line_plain(&format!("  Run: {cmd}"));
        if advice.broad {
            ui.bullet(
                "These are the development packages P3M lists; they include more than is needed, because apt could not narrow them down.",
            );
        }
    }
    if !advice.unknown.is_empty() {
        ui.bullet(&format!(
            "No system package was found for {}; `apt-file search <library>` can find it.",
            advice.unknown.join(", ")
        ));
    }
}

pub(crate) fn summary(ui: &Ui, report: &ops::SyncReport, lock: &Lockfile, start: Instant) {
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

pub(crate) fn report_json(
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
            "init-home",
            "This is your home directory. Create a rok project here?",
            false,
        )?
    {
        bail!("Cancelled. Nothing was changed.");
    }
    if let Some(parent) = dir.parent().and_then(Project::find)
        && !ui.confirm(
            "init-nested",
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
    let r = init_r(ui, &env, &installs, r.as_deref())?;

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

/// The R for a new project: the newest installed one, or the one asked for with `--r`. If it is
/// not installed, offers to install it (requirements chapter 5, init defaults).
fn init_r(
    ui: &Ui,
    env: &Env,
    installs: &[RInstallation],
    asked: Option<&str>,
) -> anyhow::Result<RInstallation> {
    use rok_rinstall::Request;

    let request: Option<Request> = asked.map(str::parse).transpose()?;
    let installed = match &request {
        None => installs
            .iter()
            .max_by(|a, b| a.version.cmp(&b.version).then(b.kind.cmp(&a.kind))),
        Some(Request::Latest) => None,
        Some(Request::Minor(m)) => ops::select_r(installs, m, None),
        Some(Request::Exact(v)) => installs.iter().find(|i| &i.version == v),
    };
    if let Some(r) = installed {
        return Ok(r.clone());
    }
    let releases = rok_rinstall::releases(&env.http, &env.dirs)?.versions;
    let wanted = request.clone().unwrap_or(Request::Latest);
    let version = wanted
        .pick(&releases)
        .ok_or_else(|| rok_rinstall::RInstallError::NoMatch(asked.unwrap_or("latest").into()))?;
    // `latest` may already be installed.
    if let Some(r) = installs.iter().find(|i| i.version == version) {
        return Ok(r.clone());
    }
    let why = match (&request, asked) {
        (None, _) => {
            ui.info("No R installation was found.");
            "the newest release".to_string()
        }
        (Some(Request::Exact(_)), _) => "as requested".to_string(),
        (_, Some(a)) => format!("the newest release matching `{a}`"),
        (_, None) => "the newest release".to_string(),
    };
    if !ui.confirm("install-r", &format!("Install R {version} ({why})?"), true)? {
        bail!("Cancelled. Nothing was changed.");
    }
    crate::rcmd::install_r(ui, env, &version, &why)
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

/// Where the code uses `name`, at most `max` places: `file:line`, or why a rule needs it.
fn usage_lines(report: &rok_core::scan::Report, name: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = report
        .used
        .get(name)
        .into_iter()
        .flatten()
        .map(|p| p.to_string())
        .chain(
            report
                .suggested
                .get(name)
                .into_iter()
                .flatten()
                .map(|(why, p)| format!("for {why} in {p}")),
        )
        .collect();
    let total = lines.len();
    lines.truncate(max);
    if total > max {
        lines.push(format!("… and {} more", total - max));
    }
    lines
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
    // Packages the code still uses: show where, and ask (the default keeps them).
    if let Ok((report, _)) =
        ops::scan_findings(&project.root, &manifest, old_lock.as_ref(), Mode::Full)
    {
        let used: Vec<&String> = packages.iter().filter(|p| report.needs(p)).collect();
        for p in &used {
            ui.warn(&format!("{p} is still used in the code:"));
            for line in usage_lines(&report, p, 5) {
                ui.bullet(&line);
            }
        }
        if !used.is_empty()
            && !ui.confirm(
                "remove-used",
                &format!(
                    "Remove {} anyway?",
                    used.iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                false,
            )?
        {
            bail!("Cancelled. Nothing was changed.");
        }
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
    let mut lock = ops::resolve_lock_with(
        env,
        &project.root,
        manifest,
        old_lock.as_ref(),
        &lock_r,
        keep,
    )?;
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
    let (lock, changes, report) =
        sync_project(ui, &env, &project, &manifest, old_lock, &r, locked)?;
    ui.changes(&changes);
    if !report.link.removed.is_empty() {
        ui.info(&format!(
            "Removed from the library: {}",
            report.link.removed.join(", ")
        ));
    }
    summary(ui, &report, &lock, start);
    // sync() reads every file (startup reads only the changed ones).
    if let Ok((_, findings)) = ops::scan_findings(&project.root, &manifest, Some(&lock), Mode::Full)
    {
        suggest_undeclared(ui, &findings, "rok add");
    }
    ui.result(report_json("sync", &changes, &report));
    Ok(())
}

/// Suggests declaring the packages the code uses but rok.toml does not declare. `add` is how
/// to add them where the person is (`rok add`, or `rok::add` in R).
pub(crate) fn suggest_undeclared(ui: &Ui, findings: &ops::ScanFindings, add: &str) {
    if findings.undeclared.is_empty() {
        return;
    }
    let n = findings.undeclared.len();
    ui.info(&format!(
        "The code uses {n} package{} that rok.toml does not declare:",
        plural(n)
    ));
    for (name, why) in &findings.undeclared {
        ui.bullet(&format!("{name} ({why})"));
    }
    let names: Vec<&str> = findings
        .undeclared
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    let command = if add.contains("::") {
        format!(
            "{add}({})",
            names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!("{add} {}", names.join(" "))
    };
    ui.bullet(&format!(
        "Add {} with `{command}`.",
        if n == 1 { "it" } else { "them" }
    ));
}

/// Updates rok.lock if rok.toml changed (unless `locked`), then makes the library match it.
fn sync_project(
    ui: &Ui,
    env: &Env,
    project: &Project,
    manifest: &Manifest,
    old_lock: Option<Lockfile>,
    r: &RInstallation,
    locked: bool,
) -> anyhow::Result<(Lockfile, Vec<ops::Change>, ops::SyncReport)> {
    // A newer rok may have a newer startup hook.
    project.refresh_activate()?;
    let current = old_lock
        .as_ref()
        .filter(|l| ops::lock_is_current(manifest, l));
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
            let lock = ops::resolve_lock(env, &project.root, manifest, old_lock.as_ref(), &lock_r)?;
            let changes = ops::diff(old_lock.as_ref(), &lock);
            (lock, changes, true)
        }
    };
    let before = lock.clone();
    let report = sync_library(ui, env, project, manifest, &mut lock, r)?;
    if relocked || lock != before {
        project.save_lock(&lock)?;
    }
    Ok((lock, changes, report))
}

/// Before `rok run`: brings rok.lock and the library up to date, saying something only if
/// anything changed.
pub(crate) fn ensure_synced(
    ui: &Ui,
    env: &Env,
    project: &Project,
    manifest: &Manifest,
    old_lock: Option<Lockfile>,
    r: &RInstallation,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let (lock, changes, report) = sync_project(ui, env, project, manifest, old_lock, r, false)?;
    let changed = !changes.is_empty()
        || report.downloaded > 0
        || !report.built.is_empty()
        || !report.link.linked.is_empty()
        || !report.link.removed.is_empty();
    if changed {
        ui.changes(&changes);
        summary(ui, &report, &lock, start);
    }
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
        let lock =
            ops::resolve_lock_with(&env, &project.root, &m, Some(&old_lock), &lock_r, &keep)?;
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
        let lock = ops::resolve_lock_with(
            &env,
            &project.root,
            &manifest,
            Some(&old_lock),
            &lock_r,
            &keep,
        )?;
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
        if !ui.confirm(
            "apply-changes",
            "Apply these changes?",
            !(moving_back || downgrades),
        )? {
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
    // System libraries (Linux), from the files on disk only: no network.
    let system = r.map(|r| {
        let paths: Vec<(String, PathBuf)> = entries
            .iter()
            .filter(|(_, i)| {
                matches!(
                    i,
                    status::Installed::Linked {
                        version: Some(_),
                        ..
                    }
                )
            })
            .map(|(n, _)| (n.clone(), library.join(n)))
            .collect();
        let packages = ops::missing_libraries(&env.platform, &paths, r);
        let advice = match &lock {
            Some(l) if !packages.is_empty() => ops::library_advice(&env, l, &packages, false),
            _ => Default::default(),
        };
        let r_missing = ops::missing_r_libraries(&env.platform, &r.r_home);
        let r_advice = if r_missing.is_empty() {
            Default::default()
        } else {
            ops::r_library_advice(&r_missing)
        };
        status::SystemCheck {
            r_missing,
            packages,
            advice,
            r_advice,
        }
    });
    // What the code uses (only changed files are read again).
    let findings = match ops::scan_findings(&project.root, &manifest, lock.as_ref(), Mode::Full) {
        Ok((_, f)) => Some(f),
        Err(e) => {
            ui.warn(&format!("The code was not scanned: {e}"));
            None
        }
    };
    let problems = status::check(&status::Inputs {
        manifest: &manifest,
        lock: lock.as_ref(),
        r,
        library: &entries,
        system: system.as_ref(),
        scan: findings.as_ref(),
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
    let Some(mut lines) = rok_core::graph::why(&lock, &declared, package) else {
        bail!("{package} is not in rok.lock.");
    };
    // Where the code uses it directly (requirements chapter 5).
    if let Ok((report, _)) = ops::scan_findings(&project.root, &manifest, Some(&lock), Mode::Full) {
        let places = usage_lines(&report, package, 5);
        if !places.is_empty() {
            lines.push("Used in the code:".to_string());
            lines.extend(places.into_iter().map(|p| format!("  {p}")));
        }
    }
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
