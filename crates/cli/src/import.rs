//! `rok import renv` and `rok export renv` (requirements chapter 7).
//!
//! A migration reproduces renv.lock exactly: renv.lock is turned into the "previous" lockfile
//! that resolution keeps versions from (CRAN versions at the snapshot dates they were current,
//! GitHub commits, and releases that a repository dropped, rebuilt from their Git origin), and
//! the result is checked package by package before anything is written.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::bail;
use rok_core::constraint::Constraint;
use rok_core::lockfile::{LockedPackage, Lockfile, ManifestCopy, Snapshot, Source, name_order};
use rok_core::manifest::{
    self, DependencySource, DependencySpec, GitRef, Manifest, ManifestDocument,
};
use rok_core::ops::{self, Env, plural};
use rok_core::par;
use rok_core::project::Project;
use rok_core::renv::{self, RenvLock, RenvPackage, RenvSource};
use rok_core::scan;
use rok_core::version::Version;
use serde_json::json;

use crate::commands::{find_project, lock_or_fail, project_r, report_json, summary, sync_library};
use crate::ui::Ui;

/// What to do with packages from sources rok cannot install yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Unsupported {
    /// Keep them as unmanaged packages: recorded by rok, installed by you.
    Unmanaged,
    /// Stop the migration.
    Abort,
}

fn absolute(dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(match dir {
        Some(d) if d.is_absolute() => d.to_path_buf(),
        Some(d) => cwd.join(d),
        None => cwd,
    })
}

pub fn import_renv(
    ui: &Ui,
    project_dir: Option<&Path>,
    file: Option<PathBuf>,
    unsupported: Option<Unsupported>,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let dir = absolute(project_dir)?;
    if dir.join(manifest::FILE_NAME).exists() {
        bail!("{} is already a rok project.", dir.display());
    }
    let path = file.unwrap_or_else(|| dir.join(renv::FILE_NAME));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let renv = RenvLock::parse(&text)?;
    let env = Env::from_env()?;
    let packages: Vec<&RenvPackage> = renv.packages.iter().filter(|p| p.name != "renv").collect();
    ui.step(&format!(
        "Reading {}: R {}, {} package{}",
        path.display(),
        renv.r,
        packages.len(),
        plural(packages.len())
    ));

    // 1. Where each package comes from.
    let mut cran: Vec<&RenvPackage> = Vec::new();
    let mut github: Vec<&RenvPackage> = Vec::new();
    let mut repos: Vec<(&RenvPackage, String, String)> = Vec::new(); // (package, alias, url)
    let mut other: Vec<(&RenvPackage, String)> = Vec::new();
    for p in &packages {
        match &p.source {
            RenvSource::Repository { repository, .. } => {
                match renv.repository_url(repository.as_deref()) {
                    Some(url) if !renv::is_cran(&url) => {
                        let alias = renv::repository_alias(&renv, &url);
                        repos.push((p, alias, url));
                    }
                    _ => cran.push(p),
                }
            }
            RenvSource::GitHub { .. } => github.push(p),
            RenvSource::Other(s) => other.push((p, s.clone())),
        }
    }
    if !other.is_empty() {
        ui.warn(&format!(
            "{} package{} come{} from sources rok cannot install yet:",
            other.len(),
            plural(other.len()),
            if other.len() == 1 { "s" } else { "" }
        ));
        for (p, s) in &other {
            ui.bullet(&format!("{} {} ({s})", p.name, p.version));
        }
        let answer = match unsupported {
            Some(a) => a,
            None => {
                let pick = ui.choose(
                    "import-unsupported",
                    "How should rok handle them?",
                    &[
                        (
                            "unmanaged".to_string(),
                            "Keep them as unmanaged packages: rok records them, you install them"
                                .to_string(),
                        ),
                        ("abort".to_string(), "Stop the migration".to_string()),
                    ],
                    "--unsupported",
                )?;
                [Unsupported::Unmanaged, Unsupported::Abort][pick]
            }
        };
        if answer == Unsupported::Abort {
            bail!("Cancelled. Nothing was changed.");
        }
    }

    // 2. The snapshot date: from the repository URLs if they have one, else the date at which
    //    the versions were current (V10). Each CRAN package also gets a date it can come from.
    ui.step("Finding the snapshot date of the versions in renv.lock");
    let dates = env.p3m.snapshot_dates(None)?.dates;
    let histories = par::map(&cran, env.jobs, |p| env.p3m.history(&p.name));
    let mut intervals: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut unavailable = Vec::new();
    for (p, h) in cran.iter().zip(histories) {
        match renv::interval(&h?, &p.version, &dates) {
            Some(i) => {
                intervals.insert(p.name.clone(), i);
            }
            None => unavailable.push(format!("{} {}", p.name, p.version)),
        }
    }
    if !unavailable.is_empty() {
        bail!(
            "These versions are in no P3M snapshot, so rok cannot reproduce them: {}\nNothing was changed.",
            unavailable.join(", ")
        );
    }
    let in_url = cran
        .iter()
        .filter_map(|p| match &p.source {
            RenvSource::Repository { repository, .. } => renv.repository_url(repository.as_deref()),
            _ => None,
        })
        .chain(renv.repositories.iter().map(|(_, u)| u.clone()))
        .find_map(|u| renv::date_in_url(&u));
    let all: Vec<(usize, usize)> = intervals.values().copied().collect();
    let date_index = match &in_url {
        Some(d) => {
            let (resolved, _) = env.p3m.resolve_snapshot(Some(d))?;
            dates
                .iter()
                .position(|x| *x == resolved)
                .unwrap_or(dates.len() - 1)
        }
        None => renv::best_date(&all, dates.len()).map_or(dates.len() - 1, |(i, _)| i),
    };
    let date = dates[date_index].clone();
    let current = all
        .iter()
        .filter(|(s, e)| (*s..*e).contains(&date_index))
        .count();
    match &in_url {
        Some(_) => ui.info(&format!("Snapshot date {date}, from the repository URL in renv.lock.")),
        None if current == all.len() => ui.info(&format!(
            "Snapshot date {date}: every CRAN version in renv.lock was current then."
        )),
        None => ui.info(&format!(
            "Snapshot date {date}: {current} of {} CRAN versions were current then; the others come from the dates they were current.",
            all.len()
        )),
    }
    let date_of = |name: &str| -> String {
        let (s, e) = intervals[name];
        if (s..e).contains(&date_index) {
            date.clone()
        } else {
            dates[e - 1].clone()
        }
    };

    // 3. The declarations: what nothing else needs, and what the code uses (requirements
    //    chapter 7). GitHub and repository packages are always declared: rok needs their source.
    let in_lock: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let used = scan::scan(&dir, &scan::builtin_rules(), scan::Mode::Full)
        .map(|r| r.used.into_keys().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let unmanaged: Vec<String> = other.iter().map(|(p, _)| p.name.clone()).collect();
    let mut declared: BTreeSet<String> = renv.roots().into_iter().collect();
    declared.extend(used.into_iter().filter(|n| in_lock.contains(n.as_str())));
    declared.extend(github.iter().map(|p| p.name.clone()));
    declared.extend(repos.iter().map(|(p, _, _)| p.name.clone()));
    declared.retain(|n| !unmanaged.contains(n));
    let mut list: Vec<&String> = declared.iter().collect();
    list.sort_by(|a, b| name_order(a, b));
    ui.info(&format!(
        "rok.toml will declare {} package{} (those no other package needs, and those the code uses):",
        list.len(),
        plural(list.len())
    ));
    for chunk in list.chunks(8) {
        ui.bullet(
            &chunk
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !ui.confirm("import-declare", "Declare these packages?", true)? {
        bail!(
            "Cancelled. Nothing was changed. (You can edit rok.toml after the migration instead.)"
        );
    }

    // 4. rok.toml.
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let minor: Version = renv.r.minor().parse()?;
    let (mut doc, _) = ManifestDocument::parse(&manifest::new_manifest_text(&name, &minor, &date))?;
    let gh = env.github();
    let mut references: BTreeMap<String, GitRef> = BTreeMap::new();
    let mut specs: Vec<(String, DependencySpec)> = Vec::new();
    for p in &github {
        let RenvSource::GitHub {
            owner,
            repo,
            reference,
            commit,
        } = &p.source
        else {
            continue;
        };
        // `@ref` in renv may be a tag or a branch; GitHub tells which. If it cannot, the
        // commit itself is declared, which reproduces it too.
        let r = match reference.as_deref() {
            None | Some("HEAD") => GitRef::DefaultBranch,
            Some(r) => gh.classify(owner, repo, r).unwrap_or_else(|e| {
                ui.warn(&format!(
                    "{owner}/{repo}: could not tell what `{r}` is ({e}); declaring commit {}.",
                    &commit[..7.min(commit.len())]
                ));
                GitRef::Rev(commit.clone())
            }),
        };
        references.insert(p.name.clone(), r.clone());
        specs.push((
            p.name.clone(),
            DependencySpec {
                source: DependencySource::GitHub {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    reference: r,
                    track: false,
                },
                env: BTreeMap::new(),
            },
        ));
    }
    for (p, alias, url) in &repos {
        doc.set_repository(alias, url);
        specs.push((
            p.name.clone(),
            DependencySpec {
                source: DependencySource::Repository {
                    alias: alias.clone(),
                    constraint: Constraint::any(),
                },
                env: BTreeMap::new(),
            },
        ));
    }
    for p in &cran {
        if declared.contains(&p.name) {
            specs.push((p.name.clone(), DependencySpec::cran(Constraint::any())));
        }
    }
    specs.sort_by(|a, b| name_order(&a.0, &b.0));
    for (name, spec) in &specs {
        doc.set_dependency(name, spec);
    }
    doc.set_unmanaged(&unmanaged);
    let manifest_text = doc.to_string();
    let manifest = Manifest::parse(&manifest_text)?;

    // 5. renv.lock as the previous lockfile, so that resolution keeps its versions.
    let mut previous: Vec<LockedPackage> = Vec::new();
    let locked = |p: &RenvPackage, source: Source| LockedPackage {
        name: p.name.clone(),
        version: p.version.clone(),
        source,
        dependencies: p.dependencies.clone(),
        sha256: None,
        env: BTreeMap::new(),
        sysreqs: BTreeMap::new(),
    };
    for p in &cran {
        previous.push(locked(
            p,
            Source::Repository {
                repository: "cran".into(),
                url: None,
                snapshot: Some(date_of(&p.name)),
                remote: None,
            },
        ));
    }
    for p in &github {
        if let RenvSource::GitHub {
            owner,
            repo,
            commit,
            ..
        } = &p.source
        {
            previous.push(locked(
                p,
                Source::GitHub {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    reference: references[&p.name].clone(),
                    commit: commit.clone(),
                },
            ));
        }
    }
    for (p, alias, url) in &repos {
        let remote = match &p.source {
            RenvSource::Repository { remote, .. } => remote.clone(),
            _ => None,
        };
        previous.push(locked(
            p,
            Source::Repository {
                repository: alias.clone(),
                url: Some(url.clone()),
                snapshot: None,
                remote,
            },
        ));
    }
    let previous = Lockfile {
        generated_by: format!("renv.lock via rok {}", env!("CARGO_PKG_VERSION")),
        r: renv.r.clone(),
        snapshot: Snapshot {
            date: date.clone(),
            repository: env.p3m.cran_base(),
        },
        manifest: ManifestCopy::default(),
        packages: previous,
    };

    // 6. Resolve, and check that every version is renv.lock's.
    let project = Project::new(&dir);
    let r = project_r(ui, &env, &manifest, Some(&previous))?;
    ui.step("Resolving dependencies");
    let mut lock = ops::resolve_lock(&env, &dir, &manifest, Some(&previous), &renv.r)?;
    let mut differ = Vec::new();
    for p in packages.iter().filter(|p| !unmanaged.contains(&p.name)) {
        match lock.package(&p.name) {
            Some(l) if l.version == p.version => {}
            Some(l) => differ.push(format!(
                "{}: renv.lock {}, rok {}",
                p.name, p.version, l.version
            )),
            None => differ.push(format!("{}: not in the result", p.name)),
        }
    }
    if !differ.is_empty() {
        bail!(
            "rok could not reproduce renv.lock exactly, so nothing was changed:\n  • {}",
            differ.join("\n  • ")
        );
    }
    let extra: Vec<String> = lock
        .packages
        .iter()
        .filter(|l| !in_lock.contains(l.name.as_str()) && l.source != Source::Unmanaged)
        .map(|l| format!("{} {}", l.name, l.version))
        .collect();
    if !extra.is_empty() {
        ui.info(&format!(
            "rok.lock also has {} package{} that renv.lock leaves out (R's recommended packages, or packages needed to build others): {}",
            extra.len(),
            plural(extra.len()),
            extra.join(", ")
        ));
    }

    // 7. Install, then write the files.
    let report = sync_library(ui, &env, &project, &manifest, &mut lock, &r)?;
    project.save(&manifest_text, &lock, false)?;
    project.take_over_from_renv()?;
    ui.success(&format!(
        "Migrated from renv: {} package{}, the same versions as renv.lock.",
        packages.len() - unmanaged.len(),
        plural(packages.len() - unmanaged.len())
    ));
    if !unmanaged.is_empty() {
        ui.info(&format!(
            "Unmanaged (install them yourself): {}",
            unmanaged.join(", ")
        ));
    }
    ui.info(
        "renv.lock and renv/ were left as they were; you can delete them. `rok undo` reverts the migration.",
    );
    summary(ui, &report, &lock, start);
    let mut result = report_json("import renv", &[], &report);
    result["snapshot"] = json!(date);
    result["declared"] = json!(list);
    result["unmanaged"] = json!(unmanaged);
    result["root"] = json!(project.root);
    result["r"] = json!(r.version.to_string());
    ui.result(result);
    Ok(())
}

pub fn export_renv(
    ui: &Ui,
    project_dir: Option<&Path>,
    output: Option<PathBuf>,
) -> anyhow::Result<()> {
    let project = find_project(project_dir)?;
    let lock = lock_or_fail(&project)?;
    let asked = output.is_some();
    let out = output.unwrap_or_else(|| project.root.join(renv::FILE_NAME));
    if !asked
        && out.exists()
        && !ui.confirm(
            "overwrite-renv-lock",
            &format!("{} exists. Replace it?", out.display()),
            false,
        )?
    {
        bail!("Cancelled. Nothing was changed. (Use `--output` to write elsewhere.)");
    }
    std::fs::write(&out, renv::export(&lock))?;
    let unmanaged = lock
        .packages
        .iter()
        .filter(|p| p.source == Source::Unmanaged)
        .count();
    let n = lock.packages.len() - unmanaged;
    ui.success(&format!(
        "Wrote {} ({n} package{}, without Hash).",
        out.display(),
        plural(n)
    ));
    if unmanaged > 0 {
        ui.info(&format!(
            "{unmanaged} unmanaged package{} left out: renv cannot restore {} from a repository.",
            plural(unmanaged),
            if unmanaged == 1 { "it" } else { "them" }
        ));
    }
    ui.result(json!({ "command": "export renv", "path": out, "packages": n }));
    Ok(())
}
