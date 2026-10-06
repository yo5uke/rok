//! Operations behind the commands: choosing R, resolving a lockfile and syncing a library.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::cache::PackageCache;
use crate::http::Http;
use crate::install::{self, Built, Context, InstallError, LinkReport, Plan, Wanted};
use crate::lockfile::{LockedPackage, Lockfile, ManifestCopy, Snapshot, Source, name_order};
use crate::manifest::{DependencySource, Manifest};
use crate::p3m::{self, P3m, P3mError};
use crate::par;
use crate::paths::{PathsError, UserDirs};
use crate::platform::Platform;
use crate::rdetect::{self, RInstallation};
use crate::resolve::{self, Request, ResolveError, SnapshotSource, SourceError};
use crate::version::Version;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Paths(#[from] PathsError),
    #[error(transparent)]
    P3m(#[from] P3mError),
    #[error(transparent)]
    Resolve(#[from] ResolveError),
    /// Boxed: installation errors carry several strings, and errors are passed around often.
    #[error(transparent)]
    Install(Box<InstallError>),
    #[error("{0}")]
    Unsupported(String),
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

    pub fn r_installations(&self) -> Vec<RInstallation> {
        rdetect::find_installations(&self.dirs, std::env::var_os("PATH"))
    }

    fn context<'a>(&'a self, r: &'a RInstallation) -> Context<'a> {
        Context {
            http: &self.http,
            p3m: &self.p3m,
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
    ManifestCopy {
        dependencies,
        constraints,
    }
}

/// Whether the lockfile was made from the manifest as it is now.
pub fn lock_is_current(manifest: &Manifest, lock: &Lockfile) -> bool {
    let (minor, patch) = manifest_r(manifest);
    lock.snapshot.date == manifest.project.snapshot
        && lock.r.minor() == minor
        && patch.is_none_or(|p| p == &lock.r)
        && lock.manifest == manifest_copy(manifest)
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
    let mut requirements = Vec::new();
    for (name, spec) in &manifest.dependencies {
        match &spec.source {
            DependencySource::Cran { constraint } => {
                requirements.push((name.clone(), constraint.clone()))
            }
            DependencySource::Repository { .. } => {
                return Err(OpError::Unsupported(format!(
                    "{name}: packages from [repositories] are not supported yet"
                )));
            }
            DependencySource::GitHub { .. } => {
                return Err(OpError::Unsupported(format!(
                    "{name}: GitHub packages are not supported yet"
                )));
            }
        }
    }
    let date = &manifest.project.snapshot;
    let mut preferred = HashMap::new();
    let mut locked = HashMap::new();
    for p in old.map(|l| l.packages.as_slice()).unwrap_or_default() {
        let Source::Repository {
            repository,
            snapshot,
        } = &p.source;
        if repository == "cran" {
            preferred.insert(p.name.clone(), p.version.clone());
            if let Some(d) = snapshot {
                locked.insert(p.name.clone(), (p.version.clone(), d.clone()));
            }
        }
    }
    let load = |d: &str| env.p3m.index(d).map_err(|e| SourceError(e.to_string()));
    // Declared constraints that exclude the snapshot's version need older releases: they are
    // taken from the past snapshots in which they were current (requirements, chapter 7).
    let index = env.p3m.index(date)?;
    let historical: HashSet<String> = requirements
        .iter()
        .filter(|(name, c)| index.get(name).is_some_and(|e| !c.matches(&e.version)))
        .map(|(name, _)| name.clone())
        .collect();
    let mut source = SnapshotSource::new(date, load, locked);
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
    };
    let resolved = resolve::resolve(&source, &request)?;

    let previous: HashMap<(&str, &Version), &Option<String>> = old
        .map(|l| {
            l.packages
                .iter()
                .map(|p| ((p.name.as_str(), &p.version), &p.sha256))
                .collect()
        })
        .unwrap_or_default();
    let known: Vec<Option<String>> = resolved
        .iter()
        .map(|r| {
            previous
                .get(&(r.name.as_str(), &r.version))
                .and_then(|h| (*h).clone())
        })
        .collect();
    let looked_up = par::map(&resolved, env.jobs, |r| {
        env.p3m.source_checksum(&r.name, &r.version).ok().flatten()
    });
    let packages = resolved
        .into_iter()
        .zip(known.into_iter().zip(looked_up))
        .map(|(r, (known, looked_up))| LockedPackage {
            name: r.name,
            version: r.version,
            source: Source::Repository {
                repository: "cran".to_string(),
                snapshot: Some(r.date),
            },
            dependencies: r.dependencies,
            sha256: known.or(looked_up),
        })
        .collect();
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

/// A change of one package between two lockfiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    pub from: Option<Version>,
    pub to: Option<Version>,
}

/// The packages added, removed or changed from `old` to `new`, sorted by name.
pub fn diff(old: Option<&Lockfile>, new: &Lockfile) -> Vec<Change> {
    let before: HashMap<&str, &Version> = old
        .map(|l| {
            l.packages
                .iter()
                .map(|p| (p.name.as_str(), &p.version))
                .collect()
        })
        .unwrap_or_default();
    let after: HashMap<&str, &Version> = new
        .packages
        .iter()
        .map(|p| (p.name.as_str(), &p.version))
        .collect();
    let mut names: Vec<&str> = before.keys().chain(after.keys()).copied().collect();
    names.sort_by(|a, b| name_order(a, b));
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| {
            let (from, to) = (before.get(n).copied(), after.get(n).copied());
            (from != to).then(|| Change {
                name: n.to_string(),
                from: from.cloned(),
                to: to.cloned(),
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
            .filter(|(_, p)| *p == Plan::Source)
            .map(|(w, _)| w)
            .collect()
    }

    pub fn downloads(&self) -> usize {
        self.items
            .iter()
            .filter(|(_, p)| matches!(p, Plan::Binary(_)))
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
    let wanted: Vec<Wanted> = lock
        .packages
        .iter()
        .map(|p| {
            let Source::Repository { snapshot, .. } = &p.source;
            Wanted {
                name: p.name.clone(),
                version: p.version.clone(),
                date: snapshot
                    .clone()
                    .unwrap_or_else(|| lock.snapshot.date.clone()),
                dependencies: p.dependencies.clone(),
                sha256: p.sha256.clone(),
                env: manifest
                    .dependency(&p.name)
                    .map(|d| d.env.clone())
                    .unwrap_or_default(),
            }
        })
        .collect();
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
            Plan::Binary(url) => binaries.push((w, url.as_str())),
            Plan::Source => {}
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

pub fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
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
