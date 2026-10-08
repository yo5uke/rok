//! Operations behind the commands: choosing R, resolving a lockfile and syncing a library.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cache::PackageCache;
use crate::constraint::Constraint;
use crate::dcf::Dependency;
use crate::github::{GitHub, GitHubError};
use crate::http::Http;
use crate::install::{self, Built, Context, InstallError, LinkReport, Plan, Wanted};
use crate::lockfile::{LockedPackage, Lockfile, ManifestCopy, Snapshot, Source, name_order};
use crate::manifest::{DependencySource, DependencySpec, GitRef, Manifest};
use crate::p3m::{self, Index, IndexEntry, P3m, P3mError};
use crate::par;
use crate::paths::{PathsError, UserDirs};
use crate::platform::{Os, Platform};
use crate::rdetect::{self, RInstallation};
use crate::repo::{self, RepoError, Repositories};
use crate::resolve::{
    self, Candidate, Origin, Request, ResolveError, Resolved, SnapshotSource, SourceError,
};
use crate::syslibs::{self, AptAdvice, SystemLibraries};
use crate::version::Version;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Paths(#[from] PathsError),
    #[error(transparent)]
    P3m(#[from] P3mError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    /// Boxed: installation errors carry several strings, and errors are passed around often.
    #[error(transparent)]
    Install(Box<InstallError>),
    #[error("{0}")]
    Message(String),
}

impl From<InstallError> for OpError {
    fn from(e: InstallError) -> Self {
        OpError::Install(Box::new(e))
    }
}

/// Shared state for operations.
pub struct Env {
    pub dirs: UserDirs,
    pub http: Http,
    pub p3m: P3m,
    pub cache: PackageCache,
    pub platform: Platform,
    pub cran: String,
    pub jobs: usize,
}

impl Env {
    /// Uses the per-user directories, the public P3M (or `ROK_P3M_URL`) and CRAN (or
    /// `ROK_CRAN_URL`).
    pub fn from_env() -> Result<Env, OpError> {
        let dirs = UserDirs::from_env()?;
        let http = Http::new();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let p3m_url = var("ROK_P3M_URL").unwrap_or_else(|| p3m::DEFAULT_URL.to_string());
        Ok(Env {
            p3m: P3m::new(&p3m_url, http.clone(), &dirs),
            cache: PackageCache::new(&dirs),
            platform: Platform::detect(),
            cran: var("ROK_CRAN_URL").unwrap_or_else(|| install::DEFAULT_CRAN.to_string()),
            jobs: 16,
            http,
            dirs,
        })
    }

    pub fn github(&self) -> GitHub<'_> {
        GitHub::new(&self.http)
    }

    pub fn r_installations(&self) -> Vec<RInstallation> {
        rdetect::find_installations(&self.dirs, std::env::var_os("PATH"))
    }

    fn context<'a>(&'a self, r: &'a RInstallation) -> Context<'a> {
        Context {
            http: &self.http,
            p3m: &self.p3m,
            repos: Repositories::new(&self.http, &self.dirs),
            github: self.github(),
            cache: &self.cache,
            platform: &self.platform,
            r,
            cran: &self.cran,
            jobs: self.jobs,
        }
    }
}

/// The R version a manifest asks for, as `(minor, exact patch if pinned)`.
pub fn manifest_r(manifest: &Manifest) -> (String, Option<&Version>) {
    let r = &manifest.project.r;
    (r.minor(), (r.parts().len() == 3).then_some(r))
}

/// Picks an installed R of minor version `minor`: `prefer` if installed, otherwise the newest
/// patch, favouring installations managed by rok.
pub fn select_r<'a>(
    installs: &'a [RInstallation],
    minor: &str,
    prefer: Option<&Version>,
) -> Option<&'a RInstallation> {
    let matching: Vec<&RInstallation> = installs
        .iter()
        .filter(|i| i.version.minor() == minor)
        .collect();
    if let Some(p) = prefer
        && let Some(exact) = matching.iter().find(|i| &i.version == p)
    {
        return Some(exact);
    }
    matching
        .into_iter()
        .max_by(|a, b| a.version.cmp(&b.version).then(b.kind.cmp(&a.kind)))
}

/// The copy of the declarations that the lockfile keeps.
pub fn manifest_copy(manifest: &Manifest) -> ManifestCopy {
    let mut constraints = BTreeMap::new();
    for (name, spec) in &manifest.dependencies {
        match &spec.source {
            DependencySource::Cran { constraint }
            | DependencySource::Repository { constraint, .. }
                if !constraint.is_any() =>
            {
                constraints.insert(name.clone(), constraint.clone());
            }
            _ => {}
        }
    }
    let mut dependencies: Vec<String> = manifest
        .dependencies
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    dependencies.sort_by(|a, b| name_order(a, b));
    let sources = manifest
        .dependencies
        .iter()
        .filter_map(|(n, spec)| source_key(spec).map(|k| (n.clone(), k)))
        .collect();
    ManifestCopy {
        dependencies,
        constraints,
        sources,
    }
}

/// A declaration's source in a canonical form, for the lockfile's copy of the manifest:
/// `None` for CRAN, `repo:<alias>`, or `github:<owner>/<repo>` followed by `#branch=…`,
/// `#tag=…` or `#rev=…` and `#track`.
pub fn source_key(spec: &DependencySpec) -> Option<String> {
    match &spec.source {
        DependencySource::Cran { .. } => None,
        DependencySource::Repository { alias, .. } => Some(format!("repo:{alias}")),
        DependencySource::GitHub {
            owner,
            repo,
            reference,
            track,
        } => {
            let mut key = format!("github:{owner}/{repo}");
            match reference {
                GitRef::DefaultBranch => {}
                GitRef::Branch(b) => key.push_str(&format!("#branch={b}")),
                GitRef::Tag(t) => key.push_str(&format!("#tag={t}")),
                GitRef::Rev(r) => key.push_str(&format!("#rev={r}")),
            }
            if *track {
                key.push_str("#track");
            }
            Some(key)
        }
    }
}

/// Whether the lockfile was made from the manifest as it is now: the same snapshot, R,
/// declarations, repository URLs and build-time environment variables.
pub fn lock_is_current(manifest: &Manifest, lock: &Lockfile) -> bool {
    let (minor, patch) = manifest_r(manifest);
    lock.snapshot.date == manifest.project.snapshot
        && lock.r.minor() == minor
        && patch.is_none_or(|p| p == &lock.r)
        && lock.manifest == manifest_copy(manifest)
        && lock.packages.iter().all(|p| {
            let env = manifest.dependency(&p.name).map(|d| &d.env);
            let env_ok = env.map_or(p.env.is_empty(), |e| *e == p.env);
            let url_ok = match &p.source {
                Source::Repository {
                    repository, url, ..
                } if repository != "cran" => manifest.repositories.get(repository) == url.as_ref(),
                _ => true,
            };
            env_ok && url_ok
        })
}

/// How much of the old lockfile a resolution keeps.
#[derive(Debug, Clone, Default)]
pub struct Keep {
    /// Packages whose locked version (or GitHub commit) is not preferred
    /// (`rok update <package>`).
    pub unlock: HashSet<String>,
    /// Ignore the old versions altogether (`rok update` without packages). GitHub packages
    /// keep their commit unless declared with `track = true`.
    pub nothing: bool,
    /// A later snapshot to offer newer versions from (`rok update <package>`, `rok add --latest`).
    pub newer: Option<String>,
    /// Packages to take from the `newer` snapshot, while the other packages new to the
    /// lockfile still come from the project's snapshot when they can (`rok add --latest`).
    pub latest: HashSet<String>,
    /// Versions to prefer, with the snapshot date to take each from (`rok r pin`: packages
    /// moved to a version that has a binary for the new R).
    pub pinned: BTreeMap<String, (Version, String)>,
}

/// Resolves the manifest into a lockfile. Versions in `old` are kept when allowed; their
/// SHA-256 values are carried over, and missing ones are looked up on P3M (case B: only the
/// current CRAN version has one).
pub fn resolve_lock(
    env: &Env,
    manifest: &Manifest,
    old: Option<&Lockfile>,
    r_version: &Version,
) -> Result<Lockfile, OpError> {
    resolve_lock_with(env, manifest, old, r_version, &Keep::default())
}

/// [`resolve_lock`] with control over what is kept from `old` (see [`Keep`]).
pub fn resolve_lock_with(
    env: &Env,
    manifest: &Manifest,
    old: Option<&Lockfile>,
    r_version: &Version,
    keep: &Keep,
) -> Result<Lockfile, OpError> {
    let date = &manifest.project.snapshot;
    let kept = if keep.nothing { None } else { old };
    let repos = Repositories::new(&env.http, &env.dirs);
    let github = env.github();

    // Packages from a repository or GitHub are offered only from there.
    let mut requirements = Vec::new();
    let mut fixed: Vec<(String, Vec<Candidate>)> = Vec::new();
    for (name, spec) in &manifest.dependencies {
        match &spec.source {
            DependencySource::Cran { constraint } => {
                requirements.push((name.clone(), constraint.clone()));
            }
            DependencySource::Repository { alias, constraint } => {
                requirements.push((name.clone(), constraint.clone()));
                let url = &manifest.repositories[alias];
                let origin = Origin::Repository {
                    alias: alias.clone(),
                    url: url.clone(),
                };
                let mut candidates = Vec::new();
                if let Some(e) = repos.source_index(url)?.get(name) {
                    candidates.push(Candidate::from_entry(e, origin.clone()));
                }
                // The repository keeps only its newest version; the locked one is reproduced
                // from the cache (requirements, chapter 7).
                if let Some(p) = kept
                    .and_then(|l| l.package(name))
                    .filter(|p| same_origin(&p.source, &origin))
                    && candidates.iter().all(|c| c.version != p.version)
                {
                    candidates.push(candidate_from_lock(p, origin.clone()));
                }
                if candidates.is_empty() {
                    return Err(OpError::Message(format!(
                        "{name} is not in the `{alias}` repository ({url})."
                    )));
                }
                fixed.push((name.clone(), candidates));
            }
            DependencySource::GitHub {
                owner,
                repo,
                reference,
                track,
            } => {
                requirements.push((name.clone(), Constraint::any()));
                // The locked commit stays unless the package is updated by name, or tracked
                // and the whole project is updated.
                let locked = old
                    .and_then(|l| l.package(name))
                    .and_then(|p| match &p.source {
                        Source::GitHub {
                            owner: o,
                            repo: r,
                            reference: re,
                            commit,
                        } if o == owner && r == repo && re == reference => Some((p, commit)),
                        _ => None,
                    });
                let candidate = match locked {
                    Some((p, commit))
                        if !(keep.unlock.contains(name) || keep.nothing && *track) =>
                    {
                        let origin = Origin::GitHub {
                            owner: owner.clone(),
                            repo: repo.clone(),
                            reference: reference.clone(),
                            commit: commit.clone(),
                        };
                        candidate_from_lock(p, origin)
                    }
                    _ => {
                        let (entry, commit) = read_github(&github, owner, repo, reference)?;
                        if entry.name != *name {
                            return Err(OpError::Message(format!(
                                "{owner}/{repo} is the package `{}`, not `{name}`.",
                                entry.name
                            )));
                        }
                        let origin = Origin::GitHub {
                            owner: owner.clone(),
                            repo: repo.clone(),
                            reference: reference.clone(),
                            commit,
                        };
                        Candidate::from_entry(&entry, origin)
                    }
                };
                fixed.push((name.clone(), vec![candidate]));
            }
        }
    }

    let mut preferred = HashMap::new();
    let mut locked = HashMap::new();
    for p in kept.map(|l| l.packages.as_slice()).unwrap_or_default() {
        if !keep.unlock.contains(&p.name) {
            preferred.insert(p.name.clone(), p.version.clone());
        }
        if let Source::Repository {
            repository,
            snapshot: Some(d),
            ..
        } = &p.source
            && repository == "cran"
        {
            locked.insert(p.name.clone(), (p.version.clone(), d.clone()));
        }
    }
    for (name, (version, d)) in &keep.pinned {
        preferred.insert(name.clone(), version.clone());
        locked.insert(name.clone(), (version.clone(), d.clone()));
    }
    let mut prefer_date = None;
    if let Some(newer) = &keep.newer
        && !keep.latest.is_empty()
    {
        let index = env.p3m.index(newer)?;
        for name in &keep.latest {
            if let Some(e) = index.get(name) {
                preferred.insert(name.clone(), e.version.clone());
            }
        }
        prefer_date = Some(date.clone());
    }

    let load = |d: &str| env.p3m.index(d).map_err(|e| SourceError(e.to_string()));
    // Declared constraints that exclude the snapshot's version need older releases: they are
    // taken from the past snapshots in which they were current (requirements, chapter 7).
    let index = env.p3m.index(date)?;
    let historical: HashSet<String> = requirements
        .iter()
        .filter(|(name, _)| fixed.iter().all(|(f, _)| f != name))
        .filter(|(name, c)| index.get(name).is_some_and(|e| !c.matches(&e.version)))
        .map(|(name, _)| name.clone())
        .collect();
    let mut source = SnapshotSource::new(date, load, locked);
    for (name, candidates) in fixed {
        source = source.with_fixed(&name, candidates);
    }
    if let Some(newer) = &keep.newer {
        source = source.with_newer(newer);
    }
    if !historical.is_empty() {
        let dates = env.p3m.snapshot_dates(Some(date))?.dates;
        source = source.with_history(dates, historical, |name: &str| {
            env.p3m
                .history(name)
                .map_err(|e| SourceError(e.to_string()))
        });
    }
    let request = Request {
        r_version: r_version.clone(),
        os: env.platform.os,
        requirements,
        preferred,
        include_linking_to: true,
        prefer_date,
    };
    let resolved = resolve::resolve(&source, &request)?;

    let previous: HashMap<&str, &LockedPackage> = old
        .map(|l| l.packages.iter().map(|p| (p.name.as_str(), p)).collect())
        .unwrap_or_default();
    let same = |r: &Resolved| {
        previous
            .get(r.name.as_str())
            .copied()
            .filter(|p| p.version == r.version && same_origin(&p.source, &r.origin))
    };
    let looked_up = par::map(&resolved, env.jobs, |r| {
        let known = same(r).is_some_and(|p| p.sha256.is_some());
        match &r.origin {
            Origin::Snapshot(_) if !known => {
                env.p3m.source_checksum(&r.name, &r.version).ok().flatten()
            }
            _ => None,
        }
    });
    let mut packages = Vec::new();
    for (r, looked_up) in resolved.iter().zip(looked_up) {
        let prev = same(r);
        let source = match &r.origin {
            Origin::Snapshot(d) => Source::Repository {
                repository: "cran".to_string(),
                url: None,
                snapshot: Some(d.clone()),
                remote: None,
            },
            Origin::Repository { alias, url } => Source::Repository {
                repository: alias.clone(),
                url: Some(url.clone()),
                snapshot: None,
                remote: prev.and_then(|p| match &p.source {
                    Source::Repository { remote, .. } => remote.clone(),
                    Source::GitHub { .. } => None,
                }),
            },
            Origin::GitHub {
                owner,
                repo,
                reference,
                commit,
            } => Source::GitHub {
                owner: owner.clone(),
                repo: repo.clone(),
                reference: reference.clone(),
                commit: commit.clone(),
            },
        };
        packages.push(LockedPackage {
            name: r.name.clone(),
            version: r.version.clone(),
            source,
            dependencies: r.dependencies.clone(),
            sha256: prev
                .and_then(|p| p.sha256.clone())
                .or_else(|| r.sha256.clone())
                .or(looked_up),
            env: manifest
                .dependency(&r.name)
                .map(|d| d.env.clone())
                .unwrap_or_default(),
            sysreqs: prev.map(|p| p.sysreqs.clone()).unwrap_or_default(),
        });
    }
    // System requirements for this distribution, from P3M (CRAN packages only). Those of
    // other distributions, recorded by collaborators, are kept; offline, the old ones stay.
    if let Some((key, distribution, release)) = env.platform.sysreqs_distro() {
        let names: Vec<&str> = packages
            .iter()
            .filter(|p| p.source.snapshot().is_some())
            .map(|p| p.name.as_str())
            .collect();
        if let Ok(found) = env.p3m.sysreqs(&names, distribution, &release) {
            for p in &mut packages {
                match found.get(&p.name).filter(|_| p.source.snapshot().is_some()) {
                    Some(reqs) => p.sysreqs.insert(key.clone(), reqs.clone()),
                    None => p.sysreqs.remove(&key),
                };
            }
        }
    }
    Ok(Lockfile {
        generated_by: format!("rok {}", env!("CARGO_PKG_VERSION")),
        r: r_version.clone(),
        snapshot: Snapshot {
            date: date.clone(),
            repository: env.p3m.cran_base(),
        },
        manifest: manifest_copy(manifest),
        packages,
    })
}

/// Whether a locked source is where a resolved version comes from (snapshot dates aside).
fn same_origin(source: &Source, origin: &Origin) -> bool {
    match (source, origin) {
        (Source::Repository { repository, .. }, Origin::Snapshot(_)) => repository == "cran",
        (
            Source::Repository {
                repository, url, ..
            },
            Origin::Repository { alias, url: u },
        ) => repository == alias && url.as_ref() == Some(u),
        (
            Source::GitHub {
                owner,
                repo,
                commit,
                ..
            },
            Origin::GitHub {
                owner: o,
                repo: r,
                commit: c,
                ..
            },
        ) => owner == o && repo == r && commit == c,
        _ => false,
    }
}

/// A candidate made from the lockfile, for a version its source no longer lists or a GitHub
/// commit that need not be looked up again. Its dependencies are known by name only.
fn candidate_from_lock(p: &LockedPackage, origin: Origin) -> Candidate {
    Candidate {
        version: p.version.clone(),
        origin,
        r_constraint: Constraint::any(),
        dependencies: p
            .dependencies
            .iter()
            .map(|d| Dependency {
                name: d.clone(),
                constraint: Constraint::any(),
            })
            .collect(),
        linking_to: Vec::new(),
        os_type: None,
        sha256: p.sha256.clone(),
    }
}

/// Resolves a GitHub reference to a commit and reads the package's DESCRIPTION there.
pub fn read_github(
    github: &GitHub,
    owner: &str,
    repo: &str,
    reference: &GitRef,
) -> Result<(IndexEntry, String), OpError> {
    let commit = github.resolve(owner, repo, reference)?;
    let text = github.description(owner, repo, &commit)?;
    let index = Index::parse_repository(&format!("{owner}/{repo}"), &text);
    match index.iter().next() {
        Some(e) => Ok((e.clone(), commit)),
        None => Err(OpError::Message(format!(
            "{owner}/{repo}: cannot read its DESCRIPTION{}",
            index
                .skipped
                .first()
                .map(|(_, why)| format!(" ({why})"))
                .unwrap_or_default()
        ))),
    }
}

/// A change of one package between two lockfiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    pub from: Option<Version>,
    pub to: Option<Version>,
    /// Where the package comes from, when that is worth showing: a source other than CRAN for
    /// an added package, or `old → new` when the source or GitHub commit changed.
    pub note: Option<String>,
}

/// Where a locked package comes from, for change lists: `CRAN`, a repository alias, or
/// `owner/repo@commit`.
pub fn origin_label(source: &Source) -> String {
    match source {
        Source::Repository { repository, .. } if repository == "cran" => "CRAN".to_string(),
        Source::Repository { repository, .. } => repository.clone(),
        Source::GitHub {
            owner,
            repo,
            commit,
            ..
        } => format!("{owner}/{repo}@{}", &commit[..commit.len().min(7)]),
    }
}

/// The packages added, removed or changed (in version or source) from `old` to `new`,
/// sorted by name.
pub fn diff(old: Option<&Lockfile>, new: &Lockfile) -> Vec<Change> {
    let entries = |l: &Lockfile| -> HashMap<String, (Version, String)> {
        l.packages
            .iter()
            .map(|p| (p.name.clone(), (p.version.clone(), origin_label(&p.source))))
            .collect()
    };
    let before = old.map(entries).unwrap_or_default();
    let after = entries(new);
    let mut names: Vec<&String> = before.keys().chain(after.keys()).collect();
    names.sort_by(|a, b| name_order(a, b));
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| {
            let (from, to) = (before.get(n), after.get(n));
            if from == to {
                return None;
            }
            let note = match (from, to) {
                (None, Some((_, s))) if s != "CRAN" => Some(s.clone()),
                (Some((_, a)), Some((_, b))) if a != b => Some(format!("{a} → {b}")),
                _ => None,
            };
            Some(Change {
                name: n.clone(),
                from: from.map(|(v, _)| v.clone()),
                to: to.map(|(v, _)| v.clone()),
                note,
            })
        })
        .collect()
}

/// How each package of a lockfile will be made available.
pub struct SyncPlan {
    pub items: Vec<(Wanted, Plan)>,
}

impl SyncPlan {
    /// Packages that must be built from source (a "heavy" sync).
    pub fn sources(&self) -> Vec<&Wanted> {
        self.items
            .iter()
            .filter(|(_, p)| matches!(p, Plan::Source { .. }))
            .map(|(w, _)| w)
            .collect()
    }

    /// Packages rebuilt from their Git origin because the repository no longer has them.
    pub fn rebuilds(&self) -> Vec<&Wanted> {
        self.items
            .iter()
            .filter(|(_, p)| *p == Plan::Source { git: true })
            .map(|(w, _)| w)
            .collect()
    }

    pub fn downloads(&self) -> usize {
        self.items
            .iter()
            .filter(|(_, p)| matches!(p, Plan::Binary { .. }))
            .count()
    }
}

/// Plans a sync of `lock` (looks packages up in the cache and on P3M).
pub fn plan_sync(
    env: &Env,
    lock: &Lockfile,
    manifest: &Manifest,
    r: &RInstallation,
) -> Result<SyncPlan, OpError> {
    let wanted = lock
        .packages
        .iter()
        .map(|p| {
            let origin = match &p.source {
                Source::Repository {
                    repository,
                    snapshot,
                    ..
                } if repository == "cran" => Origin::Snapshot(
                    snapshot
                        .clone()
                        .unwrap_or_else(|| lock.snapshot.date.clone()),
                ),
                Source::Repository {
                    repository, url, ..
                } => Origin::Repository {
                    alias: repository.clone(),
                    url: url
                        .clone()
                        .or_else(|| manifest.repositories.get(repository).cloned())
                        .ok_or_else(|| {
                            OpError::Message(format!(
                                "{}: rok.lock does not record the URL of the `{repository}` repository, and rok.toml does not declare it.",
                                p.name
                            ))
                        })?,
                },
                Source::GitHub {
                    owner,
                    repo,
                    reference,
                    commit,
                } => Origin::GitHub {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    reference: reference.clone(),
                    commit: commit.clone(),
                },
            };
            Ok(Wanted {
                name: p.name.clone(),
                version: p.version.clone(),
                origin,
                dependencies: p.dependencies.clone(),
                sha256: p.sha256.clone(),
                env: p.env.clone(),
                remote: match &p.source {
                    Source::Repository { remote, .. } => remote.clone(),
                    Source::GitHub { .. } => None,
                },
            })
        })
        .collect::<Result<Vec<Wanted>, OpError>>()?;
    let plans = env.context(r).assess(&wanted)?;
    Ok(SyncPlan {
        items: wanted.into_iter().zip(plans).collect(),
    })
}

/// What a sync did.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub downloaded: usize,
    pub built: Vec<Built>,
    pub link: LinkReport,
    /// Every package of the library with its path in the cache.
    pub paths: Vec<(String, PathBuf)>,
}

/// Carries out a plan: downloads binaries, builds sources, then links the library.
pub fn execute_sync(
    env: &Env,
    plan: &SyncPlan,
    library: &Path,
    r: &RInstallation,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<SyncReport, OpError> {
    let ctx = env.context(r);
    let mut paths: HashMap<String, PathBuf> = HashMap::new();
    let mut binaries = Vec::new();
    for (w, p) in &plan.items {
        match p {
            Plan::Cached(path) => {
                paths.insert(w.name.clone(), path.clone());
            }
            Plan::Binary { url, key, sha256 } => {
                binaries.push((w, url.as_str(), key.as_str(), sha256.as_deref()))
            }
            Plan::Source { .. } => {}
        }
    }
    if !binaries.is_empty() {
        progress(&format!(
            "Downloading {} package{}",
            binaries.len(),
            plural(binaries.len())
        ));
        paths.extend(ctx.fetch_binaries(&binaries)?);
    }
    let sources = plan.sources();
    let built = if sources.is_empty() {
        Vec::new()
    } else {
        let built = ctx.build_sources(&sources, &paths, progress)?;
        paths.extend(built.iter().map(|b| (b.name.clone(), b.path.clone())));
        built
    };
    let mut all: Vec<(String, PathBuf)> = paths.into_iter().collect();
    all.sort();
    let link = install::link(library, &all, env.cache.root())?;
    Ok(SyncReport {
        downloaded: binaries.len(),
        built,
        link,
        paths: all,
    })
}

/// Records the SHA-256 of sources that were downloaded from CRAN, when the lockfile lacks it
/// (requirements, chapter 7). Returns whether the lockfile changed.
pub fn record_built_checksums(lock: &mut Lockfile, built: &[Built]) -> bool {
    let mut changed = false;
    for b in built {
        if let (Some(h), Some(p)) = (
            &b.sha256,
            lock.packages.iter_mut().find(|p| p.name == b.name),
        ) && p.sha256.is_none()
        {
            p.sha256 = Some(h.clone());
            changed = true;
        }
    }
    changed
}

/// Records `RemoteUrl`, `RemoteSha` and `RemoteSubdir` of packages from repositories other
/// than CRAN, read from their DESCRIPTION, so a release the repository drops can be rebuilt
/// from Git (requirements, chapter 7). Returns whether the lockfile changed.
pub fn record_remotes(lock: &mut Lockfile, paths: &[(String, PathBuf)]) -> bool {
    let mut changed = false;
    for (name, path) in paths {
        if let Some(p) = lock.packages.iter_mut().find(|p| &p.name == name)
            && let Source::Repository {
                repository, remote, ..
            } = &mut p.source
            && repository != "cran"
            && let Some(mut found) = repo::remote_of(path)
        {
            found.rebuilt = remote.as_ref().is_some_and(|r| r.rebuilt);
            if remote.as_ref() != Some(&found) {
                *remote = Some(found);
                changed = true;
            }
        }
    }
    changed
}

/// Marks packages that were rebuilt from their Git origin, so the lockfile tells everyone and
/// later syncs go straight to Git. Returns whether the lockfile changed.
pub fn record_rebuilt(lock: &mut Lockfile, built: &[Built]) -> bool {
    let mut changed = false;
    for b in built.iter().filter(|b| b.git) {
        if let Some(p) = lock.packages.iter_mut().find(|p| p.name == b.name)
            && let Source::Repository {
                remote: Some(r), ..
            } = &mut p.source
            && !r.rebuilt
        {
            r.rebuilt = true;
            changed = true;
        }
    }
    changed
}

/// Packages of the lockfile that were rebuilt from Git, for messages:
/// `name version (url@sha)`.
pub fn rebuilt_packages(lock: &Lockfile) -> Vec<String> {
    lock.packages
        .iter()
        .filter_map(|p| match &p.source {
            Source::Repository {
                remote: Some(r), ..
            } if r.rebuilt => Some(format!(
                "{} {} ({}@{})",
                p.name,
                p.version,
                r.url,
                &r.sha[..r.sha.len().min(7)]
            )),
            _ => None,
        })
        .collect()
}

/// Shared libraries that packages of a library need but the system lacks (Linux), by package.
/// `paths` are the packages' directories. Empty where this cannot be checked.
pub fn missing_libraries(
    platform: &Platform,
    paths: &[(String, PathBuf)],
    r: &RInstallation,
) -> BTreeMap<String, Vec<String>> {
    if platform.os != Os::Linux {
        return BTreeMap::new();
    }
    let Some(libs) = SystemLibraries::detect(platform.arch) else {
        return BTreeMap::new();
    };
    let extra = [r.r_home.join("lib")];
    paths
        .iter()
        .filter_map(|(name, path)| {
            let missing = libs.missing(&syslibs::package_objects(path), &extra);
            (!missing.is_empty()).then(|| (name.clone(), missing))
        })
        .collect()
}

/// Shared libraries that R itself needs but the system lacks (Linux).
pub fn missing_r_libraries(platform: &Platform, r_home: &Path) -> Vec<String> {
    if platform.os != Os::Linux {
        return Vec::new();
    }
    SystemLibraries::detect(platform.arch)
        .map(|libs| libs.missing(&syslibs::r_objects(r_home), &[r_home.join("lib")]))
        .unwrap_or_default()
}

/// What to install for libraries R itself lacks (no system requirements are known for R, so
/// only apt's package names and the known toolchain libraries help).
pub fn r_library_advice(missing: &[String]) -> AptAdvice {
    syslibs::apt_advice(
        &BTreeMap::from([("R".to_string(), missing.to_vec())]),
        &BTreeMap::new(),
    )
}

/// What to install for missing libraries, from the lockfile's system requirements for this
/// distribution. With `online`, requirements the lockfile lacks (it was resolved on another
/// distribution) are looked up on P3M.
pub fn library_advice(
    env: &Env,
    lock: &Lockfile,
    missing: &BTreeMap<String, Vec<String>>,
    online: bool,
) -> AptAdvice {
    let Some((key, distribution, release)) = env.platform.sysreqs_distro() else {
        return AptAdvice {
            unknown: missing.values().flatten().cloned().collect(),
            ..Default::default()
        };
    };
    let mut sysreqs: BTreeMap<String, Vec<String>> = missing
        .keys()
        .filter_map(|n| Some((n.clone(), lock.package(n)?.sysreqs.get(&key)?.clone())))
        .collect();
    let lacking: Vec<&str> = missing
        .keys()
        .filter(|n| !sysreqs.contains_key(*n))
        .map(String::as_str)
        .collect();
    if online
        && !lacking.is_empty()
        && let Ok(found) = env.p3m.sysreqs(&lacking, distribution, &release)
    {
        for n in lacking {
            if let Some(r) = found.get(n) {
                sysreqs.insert(n.to_string(), r.clone());
            }
        }
    }
    syslibs::apt_advice(missing, &sysreqs)
}

pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lockfile::Remote;
    use crate::rdetect::RKind;

    fn r(v: &str, kind: RKind) -> RInstallation {
        RInstallation {
            version: v.parse().unwrap(),
            r_home: PathBuf::from(format!("/r/{v}/{kind:?}")),
            executable: PathBuf::from("R"),
            kind,
        }
    }

    #[test]
    fn selects_an_installed_r() {
        let installs = [
            r("4.4.1", RKind::System),
            r("4.4.2", RKind::Path),
            r("4.4.2", RKind::Managed),
            r("4.6.1", RKind::Path),
        ];
        let pick = |minor, prefer: Option<&str>| {
            let prefer = prefer.map(|p| p.parse::<Version>().unwrap());
            select_r(&installs, minor, prefer.as_ref()).map(|i| (i.version.to_string(), i.kind))
        };
        assert_eq!(pick("4.4", None), Some(("4.4.2".into(), RKind::Managed)));
        assert_eq!(
            pick("4.4", Some("4.4.1")),
            Some(("4.4.1".into(), RKind::System))
        );
        assert_eq!(
            pick("4.4", Some("4.4.0")),
            Some(("4.4.2".into(), RKind::Managed))
        );
        assert_eq!(pick("4.5", None), None);
    }

    fn lock(packages: &[(&str, &str)]) -> Lockfile {
        let mut text = String::from(
            "version = 1\ngenerated-by = \"rok\"\n[r]\nversion = \"4.6.1\"\n[snapshot]\ndate = \"2026-10-01\"\nrepository = \"x\"\n[manifest]\ndependencies = [\"a\"]\n",
        );
        for (n, v) in packages {
            text.push_str(&format!("[[package]]\nname = \"{n}\"\nversion = \"{v}\"\nsource = {{ repository = \"cran\" }}\n"));
        }
        Lockfile::parse(&text).unwrap()
    }

    #[test]
    fn diffs_lockfiles() {
        let old = lock(&[("a", "1.0"), ("b", "1.0"), ("c", "2.0")]);
        let new = lock(&[("a", "1.0"), ("B2", "1.0"), ("c", "1.5")]);
        let changes: Vec<String> = diff(Some(&old), &new)
            .iter()
            .map(|c| {
                format!(
                    "{} {:?}->{:?}",
                    c.name,
                    c.from.as_ref().map(|v| v.to_string()),
                    c.to.as_ref().map(|v| v.to_string())
                )
            })
            .collect();
        assert_eq!(
            changes,
            [
                "b Some(\"1.0\")->None",
                "B2 None->Some(\"1.0\")",
                "c Some(\"2.0\")->Some(\"1.5\")"
            ]
        );
        assert_eq!(diff(None, &new).len(), 3);
    }

    #[test]
    fn notes_source_changes() {
        let mut old = lock(&[("a", "1.0"), ("b", "1.0")]);
        let mut new = lock(&[("a", "1.0"), ("b", "1.0"), ("c", "2.0")]);
        let gh = |commit: &str| Source::GitHub {
            owner: "o".into(),
            repo: "a".into(),
            reference: GitRef::DefaultBranch,
            commit: commit.repeat(40),
        };
        old.packages[0].source = gh("1");
        new.packages[0].source = gh("2");
        new.packages[2].source = Source::Repository {
            repository: "multiverse".into(),
            url: Some("https://x".into()),
            snapshot: None,
            remote: None,
        };
        let changes = diff(Some(&old), &new);
        let notes: Vec<(&str, Option<&str>)> = changes
            .iter()
            .map(|c| (c.name.as_str(), c.note.as_deref()))
            .collect();
        assert_eq!(
            notes,
            [
                ("a", Some("o/a@1111111 → o/a@2222222")),
                ("c", Some("multiverse"))
            ]
        );
        assert_eq!(changes[0].from, changes[0].to);
    }

    #[test]
    fn keys_sources() {
        let m = Manifest::parse(
            "[project]\nname = \"p\"\nr = \"4.6\"\nsnapshot = \"2026-10-01\"\n[repositories]\nmv = \"https://x\"\n[dependencies]\na = \"*\"\nb = { repo = \"mv\" }\nc = { github = \"o/c\", branch = \"dev\", track = true }\nd = { github = \"o/d\", rev = \"abc1234\" }\n",
        )
        .unwrap();
        let copy = manifest_copy(&m);
        assert_eq!(
            copy.sources.into_iter().collect::<Vec<_>>(),
            [
                ("b".to_string(), "repo:mv".to_string()),
                ("c".to_string(), "github:o/c#branch=dev#track".to_string()),
                ("d".to_string(), "github:o/d#rev=abc1234".to_string()),
            ]
        );
    }

    #[test]
    fn stale_when_env_or_repository_url_changes() {
        let text = |env: &str, url: &str| {
            format!(
                "[project]\nname = \"p\"\nr = \"4.6\"\nsnapshot = \"2026-10-01\"\n[repositories]\nmv = \"{url}\"\n[dependencies]\na = {{ repo = \"mv\"{env} }}\n"
            )
        };
        let m = Manifest::parse(&text(", env = { X = \"1\" }", "https://x")).unwrap();
        let mut l = lock(&[("a", "1.0")]);
        l.manifest = manifest_copy(&m);
        l.packages[0].source = Source::Repository {
            repository: "mv".into(),
            url: Some("https://x".into()),
            snapshot: None,
            remote: None,
        };
        l.packages[0].env = BTreeMap::from([("X".to_string(), "1".to_string())]);
        assert!(lock_is_current(&m, &l));
        let no_env = Manifest::parse(&text("", "https://x")).unwrap();
        assert!(!lock_is_current(&no_env, &l));
        let moved = Manifest::parse(&text(", env = { X = \"1\" }", "https://y")).unwrap();
        assert!(!lock_is_current(&moved, &l));
    }

    #[test]
    fn records_remotes_of_repository_packages() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("DESCRIPTION"),
            "Package: a\nVersion: 1.0\nRemoteUrl: https://github.com/o/a\nRemoteSha: abc\n",
        )
        .unwrap();
        let mut l = lock(&[("a", "1.0")]);
        let paths = [("a".to_string(), dir)];
        assert!(
            !record_remotes(&mut l, &paths),
            "CRAN packages are left alone"
        );
        l.packages[0].source = Source::Repository {
            repository: "mv".into(),
            url: Some("https://x".into()),
            snapshot: None,
            remote: None,
        };
        assert!(record_remotes(&mut l, &paths));
        assert!(!record_remotes(&mut l, &paths), "already recorded");
        let Source::Repository { remote, .. } = &l.packages[0].source else {
            unreachable!()
        };
        assert_eq!(
            remote,
            &Some(Remote {
                url: "https://github.com/o/a".into(),
                sha: "abc".into(),
                subdir: None,
                rebuilt: false,
            })
        );
    }

    #[test]
    fn detects_stale_lockfiles() {
        let manifest = Manifest::parse("[project]\nname = \"p\"\nr = \"4.6\"\nsnapshot = \"2026-10-01\"\n[dependencies]\na = \"*\"\n").unwrap();
        let mut l = lock(&[]);
        assert!(lock_is_current(&manifest, &l));
        l.snapshot.date = "2026-09-30".into();
        assert!(!lock_is_current(&manifest, &l));
        let pinned = Manifest::parse("[project]\nname = \"p\"\nr = \"4.6.0\"\nsnapshot = \"2026-10-01\"\n[dependencies]\na = \"*\"\n").unwrap();
        assert!(!lock_is_current(&pinned, &lock(&[])));
        let more = Manifest::parse("[project]\nname = \"p\"\nr = \"4.6\"\nsnapshot = \"2026-10-01\"\n[dependencies]\na = \"< 2\"\n").unwrap();
        assert!(!lock_is_current(&more, &lock(&[])));
    }
}
