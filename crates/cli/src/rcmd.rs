//! `rok r …` (R versions) and `rok run` (requirements chapters 5 and 6).

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::bail;
use rok_core::lockfile::Lockfile;
use rok_core::manifest::Manifest;
use rok_core::ops::{self, Env, Keep, plural};
use rok_core::pin::{self, BinaryChecker, PinError};
use rok_core::rdetect::{RInstallation, RKind};
use rok_core::version::Version;
use rok_rinstall::{self as rinstall, Request};
use serde_json::json;

use crate::commands::{find_project, project_r, report_json, summary, sync_library};
use crate::ui::{Ui, changes_json};

/// How `rok r pin` handles packages without binaries for the new R.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Strategy {
    /// Move only those packages to the nearest version with a binary.
    Move,
    /// Keep every version and build those packages from source.
    Build,
    /// Move the snapshot to the nearest date where every package has a binary.
    Date,
    /// Move the snapshot to the latest date.
    Today,
}

fn home_relative(path: &Path) -> String {
    match std::env::home_dir() {
        Some(home) if path.starts_with(&home) => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

fn kind_label(kind: RKind) -> &'static str {
    match kind {
        RKind::Managed => "rok",
        RKind::Path => "PATH",
        RKind::System => "system",
    }
}

/// The R releases, warning when only an old list could be used.
fn releases(ui: &Ui, env: &Env) -> anyhow::Result<Vec<Version>> {
    let r = rinstall::releases(&env.http, &env.dirs)?;
    if r.stale {
        ui.warn("Could not reach Posit's R builds; using the list of R versions cached earlier.");
    }
    Ok(r.versions)
}

/// The R version a project needs installed, and why (requirements chapter 6).
pub(crate) fn needed_version(
    ui: &Ui,
    env: &Env,
    manifest: &Manifest,
    lock: Option<&Lockfile>,
) -> anyhow::Result<(Version, String)> {
    let (minor, pinned) = ops::manifest_r(manifest);
    if let Some(l) = lock.filter(|l| l.r.minor() == minor) {
        return Ok((l.r.clone(), "the version rok.lock records".to_string()));
    }
    if let Some(p) = pinned {
        return Ok((p.clone(), "the version rok.toml asks for".to_string()));
    }
    let v = Request::Minor(minor.clone())
        .pick(&releases(ui, env)?)
        .ok_or_else(|| anyhow::anyhow!("No release of R {minor} is available to install."))?;
    Ok((
        v,
        format!("the newest release of R {minor}, which rok.toml asks for"),
    ))
}

/// Installs R `version`, then checks that the system has the libraries it needs.
pub(crate) fn install_r(
    ui: &Ui,
    env: &Env,
    version: &Version,
    reason: &str,
) -> anyhow::Result<RInstallation> {
    let start = Instant::now();
    ui.info(&format!("Installing R {version}: {reason}."));
    let installed = rinstall::install(&env.http, &env.dirs, &env.platform, version, &|m| {
        ui.step(m)
    })?;
    let r = installed.installation;
    ui.success(&format!(
        "Installed R {version} at {} ({}) in {:.1}s",
        home_relative(
            r.r_home
                .parent()
                .and_then(Path::parent)
                .unwrap_or(&r.r_home)
        ),
        installed.build,
        start.elapsed().as_secs_f64()
    ));
    let missing = ops::missing_r_libraries(&env.platform, &r.r_home);
    if !missing.is_empty() {
        ui.warn(&format!(
            "R {version} cannot start until these system libraries are installed: {}",
            missing.join(", ")
        ));
        let advice = ops::r_library_advice(&missing);
        if let Some(cmd) = advice.command() {
            ui.line_plain(&format!("  Run: {cmd}"));
        }
        if !advice.unknown.is_empty() {
            ui.bullet(&format!(
                "`apt-file search <library>` finds the package for {}.",
                advice.unknown.join(", ")
            ));
        }
    }
    Ok(r)
}

// ---- rok r list ----

pub fn list(ui: &Ui, project_dir: Option<&Path>, all: bool) -> anyhow::Result<()> {
    let env = Env::from_env()?;
    let installs = env.r_installations();
    let project_r = find_project(project_dir).ok().and_then(|p| {
        let (_, m) = p.read_manifest().ok()?;
        let lock = p.read_lock().ok().flatten();
        let (minor, pinned) = ops::manifest_r(&m);
        ops::select_r(&installs, &minor, pinned.or(lock.as_ref().map(|l| &l.r)))
            .map(|r| r.r_home.clone())
    });
    if installs.is_empty() {
        ui.info("No R installation was found.");
    } else {
        ui.line_plain("Installed:");
        for i in &installs {
            let used = if project_r.as_ref() == Some(&i.r_home) {
                " ← this project"
            } else {
                ""
            };
            ui.bullet(&format!(
                "{} {} ({}){used}",
                i.version,
                home_relative(&i.r_home),
                kind_label(i.kind)
            ));
        }
    }
    let available = releases(ui, &env)?;
    let shown: Vec<&Version> = if all {
        available.iter().collect()
    } else {
        let mut seen = std::collections::HashSet::new();
        available
            .iter()
            .filter(|v| seen.insert(v.minor()))
            .collect()
    };
    ui.line_plain(if all {
        "Available to install:"
    } else {
        "Available to install (the newest patch of each minor version; --all lists every version):"
    });
    for line in shown.chunks(10) {
        ui.line_plain(&format!(
            "  {}",
            line.iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    ui.result(json!({
        "command": "r list",
        "installed": installs.iter().map(|i| json!({
            "version": i.version.to_string(),
            "r_home": i.r_home,
            "kind": kind_label(i.kind),
        })).collect::<Vec<_>>(),
        "available": shown.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
    }));
    Ok(())
}

// ---- rok r install ----

pub fn install(
    ui: &Ui,
    project_dir: Option<&Path>,
    version: Option<String>,
    system: bool,
) -> anyhow::Result<()> {
    let env = Env::from_env()?;
    let (version, reason) = match &version {
        Some(v) => {
            let request: Request = v.parse()?;
            let picked = request
                .pick(&releases(ui, &env)?)
                .ok_or_else(|| rinstall::RInstallError::NoMatch(v.clone()))?;
            let reason = match request {
                Request::Exact(_) => "as requested".to_string(),
                Request::Minor(m) => format!("the newest release of R {m}"),
                Request::Latest => "the newest release".to_string(),
            };
            (picked, reason)
        }
        None => match find_project(project_dir) {
            Ok(project) => {
                let (_, manifest) = project.read_manifest()?;
                let lock = project.read_lock()?;
                needed_version(ui, &env, &manifest, lock.as_ref())?
            }
            Err(_) => {
                let v = Request::Latest
                    .pick(&releases(ui, &env)?)
                    .ok_or_else(|| anyhow::anyhow!("No R release is available."))?;
                (v, "the newest release".to_string())
            }
        },
    };
    if system {
        let Some(commands) = rinstall::system_install_commands(&env.platform, &version) else {
            bail!(
                "rok knows no system-wide install command for {}.",
                env.platform
            );
        };
        ui.info(&format!(
            "To install R {version} for all users in /opt/R/{version} ({reason}), run:"
        ));
        for c in &commands {
            ui.line_plain(&format!("  {c}"));
        }
        ui.info("This needs administrator rights, so rok does not run it.");
        ui.result(
            json!({ "command": "r install", "version": version.to_string(), "system": commands }),
        );
        return Ok(());
    }
    if let Some(existing) = env
        .r_installations()
        .into_iter()
        .find(|i| i.version == version)
    {
        ui.success(&format!(
            "R {version} is already installed at {} ({}).",
            home_relative(&existing.r_home),
            kind_label(existing.kind)
        ));
        ui.result(
            json!({ "command": "r install", "version": version.to_string(), "installed": false }),
        );
        return Ok(());
    }
    let r = install_r(ui, &env, &version, &reason)?;
    ui.result(json!({
        "command": "r install",
        "version": version.to_string(),
        "installed": true,
        "r_home": r.r_home,
    }));
    Ok(())
}

// ---- rok r uninstall ----

pub fn uninstall(ui: &Ui, version: &str) -> anyhow::Result<()> {
    let version: Version = match version.parse::<Request>()? {
        Request::Exact(v) => v,
        _ => bail!("Give the exact version to remove, such as 4.4.2."),
    };
    let env = Env::from_env()?;
    let dir = rinstall::install_dir(&env.dirs, &version);
    if !dir.is_dir() {
        let other = env
            .r_installations()
            .into_iter()
            .find(|i| i.version == version);
        match other {
            Some(i) => bail!(
                "R {version} at {} was not installed by rok, so rok does not remove it.",
                i.r_home.display()
            ),
            None => bail!("R {version} is not installed."),
        }
    }
    if !ui.confirm(
        "remove-r",
        &format!("Remove R {version} from {}?", home_relative(&dir)),
        true,
    )? {
        bail!("Cancelled. Nothing was changed.");
    }
    rinstall::uninstall(&env.dirs, &version)?;
    ui.success(&format!("Removed R {version}."));
    ui.info("Projects that use it can install it again with `rok r install`.");
    ui.result(json!({ "command": "r uninstall", "version": version.to_string() }));
    Ok(())
}

// ---- rok r pin ----

pub fn pin(
    ui: &Ui,
    project_dir: Option<&Path>,
    version: &str,
    strategy: Option<Strategy>,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let project = find_project(project_dir)?;
    let (mut doc, manifest) = project.read_manifest()?;
    let old_lock = project.read_lock()?;
    let env = Env::from_env()?;
    let request: Request = version.parse()?;
    let installs = env.r_installations();

    // The new R: the newest installed one that matches, otherwise the newest release.
    let installed = installs
        .iter()
        .filter(|i| request.matches(&i.version))
        .map(|i| i.version.clone())
        .max();
    let target = match (&request, installed) {
        (Request::Latest, _) => Request::Latest
            .pick(&releases(ui, &env)?)
            .ok_or_else(|| anyhow::anyhow!("No R release is available."))?,
        (_, Some(v)) => v,
        (_, None) => request
            .pick(&releases(ui, &env)?)
            .ok_or_else(|| rinstall::RInstallError::NoMatch(version.to_string()))?,
    };
    // rok.toml keeps the minor version unless a patch version was asked for.
    let declared: Version = match &request {
        Request::Exact(v) => v.clone(),
        _ => target.minor().parse()?,
    };
    let (current_minor, _) = ops::manifest_r(&manifest);
    if manifest.project.r == declared
        && old_lock
            .as_ref()
            .is_none_or(|l| l.r.minor() != current_minor || l.r == target)
    {
        ui.info(&format!("The project already uses R {declared}."));
        return Ok(());
    }
    let moving_back = target.minor() < current_minor;
    ui.step(&format!(
        "Moving the project from R {} to R {declared}",
        manifest.project.r
    ));
    doc.set_r(&declared);

    let mut keep = Keep::default();
    let mut new_date = None;
    if let Some(old) = &old_lock
        && target.minor() != current_minor
        && let Some(checker) = BinaryChecker::new(&env, &target)
    {
        ui.step(&format!(
            "Checking P3M for binaries for R {}",
            target.minor()
        ));
        let items = pin::cran_items(old);
        let missing = pin::missing_binaries(&checker, &items)?;
        if !missing.is_empty() {
            let choice = choose_strategy(
                ui,
                &env,
                &checker,
                &manifest,
                &items,
                &missing,
                &target,
                !moving_back,
                strategy,
            )?;
            match choice {
                Choice::Move(pinned) => {
                    keep.pinned = pinned;
                    keep.newer = Some(latest_date(&env)?);
                }
                Choice::Build => {}
                Choice::Date(d) | Choice::Today(d) => {
                    keep.nothing = true;
                    new_date = Some(d);
                }
            }
        }
    }
    if let Some(d) = &new_date {
        doc.set_snapshot(d);
    }
    let new_text = doc.to_string();
    let new_manifest = Manifest::parse(&new_text)?;

    ui.step("Resolving dependencies");
    let lock = ops::resolve_lock_with(
        &env,
        &project.root,
        &new_manifest,
        old_lock.as_ref(),
        &target,
        &keep,
    )?;
    if let Some(checker) = BinaryChecker::new(&env, &target) {
        let still = pin::missing_binaries(&checker, &pin::cran_items(&lock))?;
        if !still.is_empty() {
            ui.info(&format!(
                "{} package{} will be built from source: {}",
                still.len(),
                plural(still.len()),
                still.join(", ")
            ));
        }
    }
    let changes = ops::diff(old_lock.as_ref(), &lock);
    if changes.is_empty() {
        ui.info("No package versions change.");
    } else {
        ui.changes(&changes);
    }
    let downgrades = changes
        .iter()
        .any(|c| matches!((&c.from, &c.to), (Some(f), Some(t)) if t < f));
    if !ui.confirm(
        "apply-changes",
        &format!("Move the project to R {declared}?"),
        !(moving_back || downgrades),
    )? {
        bail!("Cancelled. Nothing was changed.");
    }

    // Install the new R if needed, then sync its library before saving anything.
    let r = match ops::select_r(&installs, &target.minor(), Some(&target)) {
        Some(r) => Some(r.clone()),
        None if ui.confirm(
            "install-r",
            &format!("R {target} is not installed. Install it now?"),
            true,
        )? =>
        {
            Some(install_r(ui, &env, &target, "the project moves to it")?)
        }
        None => None,
    };
    let mut lock = lock;
    let report = match &r {
        Some(r) => Some(sync_library(
            ui,
            &env,
            &project,
            &new_manifest,
            &mut lock,
            r,
        )?),
        None => None,
    };
    project.save(&new_text, &lock, true)?;
    if let Some(report) = &report {
        summary(ui, report, &lock, start);
    } else {
        ui.success(&format!("The project now uses R {declared}."));
        ui.info(&format!(
            "Install R {target} with `rok r install`, then run `rok sync`."
        ));
    }
    ui.info(&format!(
        "R sessions that are already running still use their R; start a new session with R {target} (in an IDE, switch its R)."
    ));
    let mut result = match &report {
        Some(rep) => report_json("r pin", &changes, rep),
        None => json!({ "command": "r pin", "changes": changes_json(&changes) }),
    };
    result["r"] = json!(target.to_string());
    if let Some(d) = new_date {
        result["snapshot"] = json!(d);
    }
    ui.result(result);
    Ok(())
}

enum Choice {
    Move(std::collections::BTreeMap<String, (Version, String)>),
    Build,
    Date(String),
    Today(String),
}

impl Choice {
    /// The `--strategy` value that picks this choice.
    fn value(&self) -> &'static str {
        match self {
            Choice::Move(_) => "move",
            Choice::Build => "build",
            Choice::Date(_) => "date",
            Choice::Today(_) => "today",
        }
    }
}

fn latest_date(env: &Env) -> anyhow::Result<String> {
    Ok(env.p3m.resolve_snapshot(None)?.0)
}

/// Shows the packages without binaries and the candidates (requirements chapter 6), and
/// returns the chosen one.
#[allow(clippy::too_many_arguments)]
fn choose_strategy(
    ui: &Ui,
    env: &Env,
    checker: &BinaryChecker,
    manifest: &Manifest,
    items: &[pin::Item],
    missing: &[String],
    target: &Version,
    prefer_newer: bool,
    strategy: Option<Strategy>,
) -> anyhow::Result<Choice> {
    let minor = target.minor();
    ui.warn(&format!(
        "{} package{} {} no binary for R {minor} on {} at the locked version:",
        missing.len(),
        plural(missing.len()),
        if missing.len() == 1 { "has" } else { "have" },
        env.platform
    ));
    for (n, v, _) in items.iter().filter(|(n, _, _)| missing.contains(n)) {
        ui.bullet(&format!("{n} {v}"));
    }
    let dates = env.p3m.snapshot_dates(None)?.dates;
    let latest = dates.last().cloned().unwrap_or_default();
    let wanted = |s: Strategy| strategy.is_none() || strategy == Some(s);

    let mut options: Vec<(String, Choice)> = Vec::new();
    if wanted(Strategy::Move) {
        ui.step("Looking for the nearest versions with binaries");
        let allowed = |name: &str, v: &Version| {
            manifest.dependency(name).is_none_or(|d| match &d.source {
                rok_core::manifest::DependencySource::Cran { constraint } => constraint.matches(v),
                _ => true,
            })
        };
        let found =
            pin::nearest_versions(env, checker, items, missing, &dates, &allowed, prefer_newer)?;
        let moves: Vec<String> = found
            .iter()
            .map(|(n, (v, _))| {
                let from = items
                    .iter()
                    .find(|(m, _, _)| m == n)
                    .map(|(_, v, _)| v.to_string())
                    .unwrap_or_default();
                format!("{n} {from} → {v}")
            })
            .collect();
        let unmoved: Vec<&String> = missing.iter().filter(|n| !found.contains_key(*n)).collect();
        let mut text = format!(
            "Keep the snapshot; move only these packages to the nearest version with a binary: {}",
            if moves.is_empty() {
                "none found".to_string()
            } else {
                moves.join(", ")
            }
        );
        if !unmoved.is_empty() {
            text.push_str(&format!(
                " ({} built from source)",
                unmoved
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        options.push((text, Choice::Move(found)));
    }
    if wanted(Strategy::Build) {
        options.push((
            format!(
                "Keep every version; build {} package{} from source",
                missing.len(),
                plural(missing.len())
            ),
            Choice::Build,
        ));
    }
    if wanted(Strategy::Date) {
        ui.step("Looking for the nearest snapshot date where every package has a binary");
        let names: Vec<String> = items.iter().map(|(n, _, _)| n.clone()).collect();
        match pin::nearest_date(
            env,
            checker,
            &names,
            &manifest.project.snapshot,
            &dates,
            prefer_newer,
        ) {
            Ok(Some(d)) => options.push((
                format!(
                    "Move the snapshot to {d}, the nearest date where every package has a binary"
                ),
                Choice::Date(d),
            )),
            Ok(None) => {
                ui.info("No snapshot date within 24 months has binaries for every package.")
            }
            Err(PinError::Budget) => ui.info(&PinError::Budget.to_string()),
            Err(e) => return Err(e.into()),
        }
    }
    if wanted(Strategy::Today) && latest != manifest.project.snapshot {
        options.push((
            format!("Move the snapshot to the latest date ({latest})"),
            Choice::Today(latest.clone()),
        ));
    }
    ui.info(&format!(
        "Checked {} package version{} on P3M.",
        checker.requests(),
        plural(checker.requests())
    ));
    if options.is_empty() {
        bail!("The chosen strategy found no candidate; try another `--strategy`.");
    }
    let labelled: Vec<(String, String)> = options
        .iter()
        .map(|(t, c)| (c.value().to_string(), t.clone()))
        .collect();
    let i = if options.len() == 1 && strategy.is_some() {
        0
    } else {
        ui.choose(
            "strategy",
            "How should these packages be handled?",
            &labelled,
            "--strategy",
        )?
    };
    Ok(options.swap_remove(i).1)
}

// ---- rok run ----

pub fn run(
    ui: &Ui,
    project_dir: Option<&Path>,
    r: Option<String>,
    args: &[String],
) -> anyhow::Result<ExitCode> {
    let env = Env::from_env()?;
    let installs = env.r_installations();
    let wanted = r.as_deref().map(str::parse::<Request>).transpose()?;
    let pick = |req: &Request| -> anyhow::Result<RInstallation> {
        installs
            .iter()
            .filter(|i| req.matches(&i.version))
            .max_by(|a, b| a.version.cmp(&b.version).then(b.kind.cmp(&a.kind)))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No installed R matches `{}`; install it with `rok r install {}`.",
                    r.as_deref().unwrap_or(""),
                    r.as_deref().unwrap_or("")
                )
            })
    };
    let mut cmd;
    match find_project(project_dir) {
        Ok(project) => {
            let (_, manifest) = project.read_manifest()?;
            let old_lock = project.read_lock()?;
            let project_r = project_r(ui, &env, &manifest, old_lock.as_ref())?;
            crate::commands::ensure_synced(ui, &env, &project, &manifest, old_lock, &project_r)?;
            let r = match &wanted {
                Some(req) => pick(req)?,
                None => project_r.clone(),
            };
            if r.version.minor() != project_r.version.minor() {
                ui.warn(&format!(
                    "The project uses R {}; its packages are not installed for R {}.",
                    project_r.version.minor(),
                    r.version
                ));
            }
            let library = project.library(&r.version.minor(), &env.platform);
            cmd = std::process::Command::new(r.rscript());
            // Only the project library and R's own: no user or site libraries (as activate.R).
            let none = project.root.join(".rok").join("none");
            cmd.env("R_LIBS", &library)
                .env("R_LIBS_USER", &none)
                .env("R_LIBS_SITE", &none);
        }
        Err(_) => {
            let r = match &wanted {
                Some(req) => pick(req)?,
                None => pick(&Request::Latest)?,
            };
            cmd = std::process::Command::new(r.rscript());
        }
    }
    let status = cmd.args(args).status()?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(c) => ExitCode::from(u8::try_from(c).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}
