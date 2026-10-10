//! Making a project library match a lockfile.
//!
//! Each package is looked up in the global cache. A missing one is either downloaded as a P3M
//! binary and extracted (no R involved), or built from CRAN's source with `R CMD INSTALL`. The
//! project library then gets a link to each cached package (a symbolic link; a junction on
//! Windows). Everything that can fail slowly (downloads, builds) happens before the library is
//! touched.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use md5::Md5;
use sha2::{Digest, Sha256};

use crate::cache::{CacheError, PackageCache};
use crate::fsutil;
use crate::github::{self, GitHub, GitHubError};
use crate::http::{Http, HttpError};
use crate::lockfile::Remote;
use crate::p3m::{Index, P3m, P3mError};
use crate::par;
use crate::platform::{Arch, Os, Platform};
use crate::rdetect::RInstallation;
use crate::repo::{self, RepoError, Repositories};
use crate::resolve::Origin;
use crate::transfer::{Group, Transfers};
use crate::version::Version;

/// The CRAN mirror used for source packages.
pub const DEFAULT_CRAN: &str = "https://cloud.r-project.org";

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    P3m(#[from] P3mError),
    #[error(transparent)]
    Cache(#[from] CacheError),
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{name} {version} is not in the {date} snapshot.")]
    NotInSnapshot {
        name: String,
        version: Version,
        date: String,
    },
    #[error(transparent)]
    ChecksumMismatch(Box<Mismatch>),
    #[error("{name} {version}: the source is not available from {from}.")]
    SourceUnavailable {
        name: String,
        version: Version,
        from: String,
    },
    #[error(transparent)]
    Build(Box<BuildFailure>),
}

/// A download whose checksum is not the expected one.
#[derive(Debug, thiserror::Error)]
#[error(
    "{name} {version}: the downloaded file's {algorithm} is {actual}, not the expected {expected}."
)]
pub struct Mismatch {
    pub name: String,
    pub version: Version,
    pub algorithm: &'static str,
    pub expected: String,
    pub actual: String,
}

/// A package that failed to build from source.
#[derive(Debug, thiserror::Error)]
#[error("Failed to build {name} {version} from source{}.\nThe log is in {}.", hint.as_ref().map(|h| format!(" ({h})")).unwrap_or_default(), log.display())]
pub struct BuildFailure {
    pub name: String,
    pub version: Version,
    pub log: PathBuf,
    pub hint: Option<String>,
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A package the library must contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub name: String,
    pub version: Version,
    /// Where to get it.
    pub origin: Origin,
    pub dependencies: Vec<String>,
    /// SHA-256 of the source tarball, when the lockfile records it.
    pub sha256: Option<String>,
    /// Environment variables for building from source.
    pub env: BTreeMap<String, String>,
    /// For packages from a repository: the Git origin to rebuild from if the repository no
    /// longer has this release.
    pub remote: Option<Remote>,
}

/// Everything installation needs.
pub struct Context<'a> {
    pub http: &'a Http,
    pub p3m: &'a P3m,
    pub repos: Repositories<'a>,
    pub github: GitHub<'a>,
    pub cache: &'a PackageCache,
    pub platform: &'a Platform,
    pub r: &'a RInstallation,
    pub cran: &'a str,
    pub jobs: usize,
}

/// What a download is checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    /// SHA-256: CRAN sources in rok.lock, and the indexes of R-multiverse and r-universe.
    Sha256(String),
    /// MD5: P3M's Windows index (`Hash`) and CRAN's (`MD5sum`).
    Md5(String),
}

/// How a package will be made available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Already in the cache.
    Cached(PathBuf),
    /// A binary at `url`, cached under `key`, checked against `checksum` when the index has one.
    Binary {
        url: String,
        key: String,
        checksum: Option<Checksum>,
    },
    /// Must be built from source; `git` when the repository no longer has the release, so it
    /// is rebuilt from its Git origin.
    Source { git: bool },
}

/// A built source package.
#[derive(Debug, Clone)]
pub struct Built {
    pub name: String,
    pub path: PathBuf,
    /// SHA-256 of the source tarball, if it came from CRAN (P3M's source differs from CRAN's).
    pub sha256: Option<String>,
    /// Rebuilt from the Git origin instead of the repository's release.
    pub git: bool,
}

/// A downloaded source: a tarball, or a directory for Git sources.
struct Fetched {
    path: PathBuf,
    sha256: Option<String>,
    git: bool,
}

fn short_hash(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))[..8].to_string()
}

impl Context<'_> {
    /// The cache key of P3M binaries for this R and platform (`4.6-noble`, `4.6-noble-arm64`,
    /// `4.6-win`, `4.6-macos-arm64`, `4.6-macos`), matching P3M's `x-package-binary-tag`.
    /// `None` when P3M has no binaries for this machine.
    pub fn binary_key(&self) -> Option<String> {
        let minor = self.r.version.minor();
        match (self.platform.os, self.platform.arch) {
            (Os::Windows, Arch::X86_64) => Some(format!("{minor}-win")),
            (Os::MacOs, Arch::Aarch64) => Some(format!("{minor}-macos-arm64")),
            (Os::MacOs, Arch::X86_64) => Some(format!("{minor}-macos")),
            (Os::Linux, arch) => {
                let distro = self.platform.p3m_linux_name()?;
                let arm = if arch == Arch::Aarch64 { "-arm64" } else { "" };
                Some(format!("{minor}-{distro}{arm}"))
            }
            _ => None,
        }
    }

    /// Where another repository keeps binaries for this R and platform, if it can have any.
    fn repo_binary_contrib(&self, url: &str) -> Option<String> {
        let minor = self.r.version.minor();
        match self.platform.os {
            Os::Windows => Some(repo::windows_contrib(url, &minor)),
            Os::Linux => Some(repo::linux_contrib(
                url,
                self.platform.p3m_linux_name()?,
                self.platform.arch.r_name(),
                &minor,
            )),
            Os::MacOs => None,
        }
    }

    /// The cache key of binaries from another repository: the same package name and version
    /// may differ between repositories, so the repository is part of the key.
    fn repo_binary_key(&self, url: &str) -> Option<String> {
        Some(format!("{}-repo-{}", self.binary_key()?, short_hash(url)))
    }

    /// The cache key of packages built here. Build-time environment variables change the
    /// result, so they are part of the key.
    pub fn source_key(&self, env: &BTreeMap<String, String>) -> String {
        let mut key = format!(
            "{}-{}-source",
            self.r.version.minor(),
            self.platform.library_tag()
        );
        if !env.is_empty() {
            let mut h = Sha256::new();
            for (k, v) in env {
                h.update(k.as_bytes());
                h.update(b"=");
                h.update(v.as_bytes());
                h.update(b"\n");
            }
            key.push('-');
            key.push_str(&hex(&h.finalize())[..8]);
        }
        key
    }

    /// The cache key of a package built here from its origin.
    fn build_key(&self, w: &Wanted) -> String {
        let base = self.source_key(&w.env);
        match &w.origin {
            Origin::Snapshot(_) => base,
            Origin::Repository { url, .. } => format!("{base}-repo-{}", short_hash(url)),
            Origin::GitHub { commit, .. } => format!("{base}{}", github_key_suffix(commit)),
            Origin::Unmanaged => base,
        }
    }

    /// The cached copy of a package, if any (binary first).
    pub fn cached(&self, w: &Wanted) -> Option<PathBuf> {
        let binary = match &w.origin {
            Origin::Snapshot(_) => self.binary_key(),
            Origin::Repository { url, .. } => self.repo_binary_key(url),
            Origin::GitHub { .. } | Origin::Unmanaged => None,
        };
        binary
            .and_then(|k| self.cache.get(&w.name, &w.version, &k))
            .or_else(|| self.cache.get(&w.name, &w.version, &self.build_key(w)))
            .or_else(|| {
                w.remote.as_ref().and_then(|_| {
                    self.cache
                        .get(&w.name, &w.version, &git_key(&self.build_key(w)))
                })
            })
    }

    /// The snapshot date of a CRAN package that is not in the cache yet.
    fn uncached_date<'w>(&self, w: &'w Wanted) -> Option<&'w str> {
        match &w.origin {
            Origin::Snapshot(date) if self.cached(w).is_none() => Some(date.as_str()),
            _ => None,
        }
    }

    /// Decides how each package will be made available. Packages not in the cache are looked
    /// up in parallel: on P3M, whose redirect says whether it has a binary for this R (on
    /// Windows and macOS, P3M's binary index of the date says so, with the file's MD5); in a
    /// repository's binary index (r-universe); GitHub packages are always built.
    pub fn assess(&self, wanted: &[Wanted]) -> Result<Vec<Plan>, InstallError> {
        let distro = self.platform.p3m_linux_name();
        let ua = self.platform.r_user_agent(&self.r.version);
        let minor = self.r.version.minor();
        // On Windows and macOS: where P3M keeps binaries, and its index for each snapshot date
        // that is needed, loaded once.
        let binary_dir = match self.binary_key() {
            Some(_) if wanted.iter().any(|w| self.uncached_date(w).is_some()) => self
                .p3m
                .binary_dir(self.platform.os, self.platform.arch, &minor)?,
            _ => None,
        };
        let mut indexes: HashMap<&str, Index> = HashMap::new();
        if let Some(dir) = &binary_dir {
            for date in wanted.iter().filter_map(|w| self.uncached_date(w)) {
                if !indexes.contains_key(date) {
                    indexes.insert(date, self.p3m.binary_index(date, dir)?);
                }
            }
        }
        let results = par::map(wanted, self.jobs, |w| -> Result<Plan, InstallError> {
            if let Some(path) = self.cached(w) {
                return Ok(Plan::Cached(path));
            }
            match &w.origin {
                Origin::Snapshot(date) if let Some(dir) = &binary_dir => {
                    let entry = indexes
                        .get(date.as_str())
                        .and_then(|i| i.get(&w.name))
                        .filter(|e| e.version == w.version);
                    Ok(match entry {
                        Some(e) => Plan::Binary {
                            url: self.p3m.binary_url(date, dir, &w.name, &w.version),
                            key: self.binary_key().expect("checked above"),
                            checksum: e.md5.clone().map(Checksum::Md5),
                        },
                        None => Plan::Source { git: false },
                    })
                }
                Origin::Snapshot(_) if distro.is_none() => Ok(Plan::Source { git: false }),
                Origin::Snapshot(date) => {
                    let distro = distro.expect("checked above");
                    let url = self
                        .p3m
                        .linux_package_url(distro, date, &w.name, &w.version);
                    let head = self.http.get_headers(&url, Some(&ua))?;
                    match head.status {
                        404 => Err(InstallError::NotInSnapshot {
                            name: w.name.clone(),
                            version: w.version.clone(),
                            date: date.clone(),
                        }),
                        200..=399 if head.header("x-package-type") == Some("binary") => {
                            Ok(Plan::Binary {
                                url: head.header("location").unwrap_or(&url).to_string(),
                                key: self
                                    .binary_key()
                                    .expect("P3M has binaries for this machine"),
                                checksum: None,
                            })
                        }
                        200..=399 => Ok(Plan::Source { git: false }),
                        status => Err(HttpError::Status { url, status }.into()),
                    }
                }
                Origin::Repository { url, .. } => {
                    let rebuilt = w.remote.as_ref().is_some_and(|r| r.rebuilt);
                    if !rebuilt
                        && let Some(contrib) = self.repo_binary_contrib(url)
                        && let Some(key) = self.repo_binary_key(url)
                    {
                        let index = self.repos.binary_index(url, &contrib)?;
                        if let Some(e) = index
                            .as_ref()
                            .and_then(|i| repo::entry(i, &w.name, &w.version))
                            .filter(|e| e.built)
                        {
                            let ext = if self.platform.os == Os::Windows {
                                "zip"
                            } else {
                                "tar.gz"
                            };
                            return Ok(Plan::Binary {
                                url: repo::file_url(
                                    &contrib,
                                    &w.name,
                                    &w.version,
                                    e.path.as_deref(),
                                    ext,
                                ),
                                key,
                                checksum: e
                                    .sha256
                                    .clone()
                                    .map(Checksum::Sha256)
                                    .or_else(|| e.md5.clone().map(Checksum::Md5)),
                            });
                        }
                    }
                    // A release the repository no longer has is rebuilt from its Git origin.
                    let listed = !rebuilt && {
                        let index = self.repos.source_index(url)?;
                        repo::entry(&index, &w.name, &w.version).is_some()
                    };
                    match (listed, &w.remote) {
                        (true, _) => Ok(Plan::Source { git: false }),
                        (false, Some(_)) => Ok(Plan::Source { git: true }),
                        (false, None) => Err(InstallError::SourceUnavailable {
                            name: w.name.clone(),
                            version: w.version.clone(),
                            from: format!(
                                "{url} any more, and it is not in the cache (rok.lock records no Git origin to rebuild it from)"
                            ),
                        }),
                    }
                }
                Origin::GitHub { .. } => Ok(Plan::Source { git: false }),
                Origin::Unmanaged => Err(unmanaged(w)),
            }
        });
        results.into_iter().collect()
    }

    /// Downloads and extracts binaries in parallel. Returns (name, cached path) pairs.
    pub fn fetch_binaries(
        &self,
        items: &[(&Wanted, &str, &str, Option<&Checksum>)],
        transfers: &dyn Transfers,
    ) -> Result<Vec<(String, PathBuf)>, InstallError> {
        let results = par::map(
            items,
            self.jobs,
            |(w, url, key, checksum)| -> Result<(String, PathBuf), InstallError> {
                let mut bytes = self.http.get_tracked(url, &w.name, transfers)?;
                // P3M's index and files can briefly disagree (V2): try once more.
                if check_checksum(w, &bytes, *checksum).is_err() {
                    bytes = self.http.get_tracked(url, &w.name, transfers)?;
                }
                check_checksum(w, &bytes, *checksum)?;
                let path = self.cache.insert_binary(&w.name, &w.version, key, &bytes)?;
                transfers.finish(&w.name);
                Ok((w.name.clone(), path))
            },
        );
        results.into_iter().collect()
    }

    /// Builds packages from source, in dependency order, into the cache. `available` maps every
    /// other package of the library to its cached path; builds can use them.
    pub fn build_sources(
        &self,
        items: &[&Wanted],
        available: &HashMap<String, PathBuf>,
        transfers: &dyn Transfers,
        progress: &(dyn Fn(&str) + Sync),
    ) -> Result<Vec<Built>, InstallError> {
        let tmp = self.cache.temp_dir()?;
        let what = format!(
            "{} source package{}",
            items.len(),
            if items.len() == 1 { "" } else { "s" }
        );
        let group = Group::begin(transfers, &what, items.len());
        let sources = par::map(items, self.jobs, |w| {
            let fetched = self.download_source(w, tmp.path(), transfers)?;
            transfers.finish(&w.name);
            Ok(fetched)
        });
        let mut sources: HashMap<&str, Fetched> = items
            .iter()
            .map(|w| w.name.as_str())
            .zip(sources)
            .map(|(n, r): (_, Result<Fetched, InstallError>)| r.map(|t| (n, t)))
            .collect::<Result<_, _>>()?;
        group.complete();

        let mut known = available.clone();
        let mut out = Vec::new();
        for w in build_order(items) {
            progress(&format!("Building {} {} from source", w.name, w.version));
            let fetched = sources
                .remove(w.name.as_str())
                .expect("every item was downloaded");
            if fetched.git {
                progress(&format!(
                    "{} {} is no longer available from its repository; rebuilding it from Git",
                    w.name, w.version
                ));
            }
            let path = self.build_one(w, &fetched.path, &known, fetched.git)?;
            known.insert(w.name.clone(), path.clone());
            out.push(Built {
                name: w.name.clone(),
                path,
                sha256: fetched.sha256,
                git: fetched.git,
            });
        }
        Ok(out)
    }

    /// Downloads a package's source into `dir`: a tarball, or for Git sources a directory. The
    /// tarball's SHA-256 is returned when it can be recorded in the lockfile.
    fn download_source(
        &self,
        w: &Wanted,
        dir: &Path,
        transfers: &dyn Transfers,
    ) -> Result<Fetched, InstallError> {
        match &w.origin {
            Origin::Unmanaged => Err(unmanaged(w)),
            Origin::Snapshot(date) => {
                let (path, sha256) = self.download_cran_source(w, date, dir, transfers)?;
                Ok(Fetched {
                    path,
                    sha256,
                    git: false,
                })
            }
            Origin::Repository { url, .. } => {
                if !w.remote.as_ref().is_some_and(|r| r.rebuilt) {
                    let index = self.repos.source_index(url)?;
                    if let Some(e) = repo::entry(&index, &w.name, &w.version) {
                        let file_url = repo::file_url(
                            &format!("{url}/src/contrib"),
                            &w.name,
                            &w.version,
                            e.path.as_deref(),
                            "tar.gz",
                        );
                        let bytes = self.http.get_tracked(&file_url, &w.name, transfers)?;
                        check_sha256(w, &bytes, e.sha256.as_deref())?;
                        match check_sha256(w, &bytes, w.sha256.as_deref()) {
                            Ok(()) => {
                                let path = dir.join(format!("{}_{}.tar.gz", w.name, w.version));
                                std::fs::write(&path, &bytes).map_err(io_err(&path))?;
                                return Ok(Fetched {
                                    path,
                                    sha256: Some(hex(&Sha256::digest(&bytes))),
                                    git: false,
                                });
                            }
                            // The repository replaced the release that rok.lock records: rebuild
                            // the recorded commit instead (requirements, chapter 7).
                            Err(_) if w.remote.is_some() => {}
                            Err(e) => return Err(e),
                        }
                    }
                }
                let Some(remote) = &w.remote else {
                    return Err(InstallError::SourceUnavailable {
                        name: w.name.clone(),
                        version: w.version.clone(),
                        from: format!("{url} any more, and it is not in the cache"),
                    });
                };
                let Some((owner, repo)) = github::parse_url(&remote.url) else {
                    return Err(InstallError::SourceUnavailable {
                        name: w.name.clone(),
                        version: w.version.clone(),
                        from: format!(
                            "{url} any more, and its Git origin ({}) is not on GitHub, so it cannot be rebuilt",
                            remote.url
                        ),
                    });
                };
                let path = self.unpack_github(
                    w,
                    &owner,
                    &repo,
                    &remote.sha,
                    remote.subdir.as_deref(),
                    dir,
                )?;
                Ok(Fetched {
                    path,
                    sha256: None,
                    git: true,
                })
            }
            Origin::GitHub {
                owner,
                repo,
                commit,
                ..
            } => Ok(Fetched {
                path: self.unpack_github(w, owner, repo, commit, None, dir)?,
                sha256: None,
                git: false,
            }),
        }
    }

    /// Downloads a GitHub commit and returns the package's directory, renamed to the package's
    /// name as R expects (GitHub's top directory is `<repo>-<commit>`).
    fn unpack_github(
        &self,
        w: &Wanted,
        owner: &str,
        repo: &str,
        commit: &str,
        subdir: Option<&str>,
        dir: &Path,
    ) -> Result<PathBuf, InstallError> {
        let unavailable = |why: &str| InstallError::SourceUnavailable {
            name: w.name.clone(),
            version: w.version.clone(),
            from: format!("{owner}/{repo} at {commit} ({why})"),
        };
        if let Some(d) = subdir
            && Path::new(d)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(unavailable(&format!("invalid subdirectory `{d}`")));
        }
        let bytes = self.github.tarball(owner, repo, commit)?;
        let unpack = dir.join(format!("gh-{}", w.name));
        tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()))
            .unpack(&unpack)
            .map_err(io_err(&unpack))?;
        let top = std::fs::read_dir(&unpack)
            .map_err(io_err(&unpack))?
            .flatten()
            .find(|e| e.path().is_dir())
            .map(|e| e.path())
            .ok_or_else(|| unavailable("the download is empty"))?;
        let package = match subdir {
            Some(d) => top.join(d),
            None => top,
        };
        if !package.join("DESCRIPTION").is_file() {
            return Err(unavailable(&match subdir {
                Some(d) => format!("no DESCRIPTION in `{d}`"),
                None => "no DESCRIPTION at the top".to_string(),
            }));
        }
        let source = unpack.join(&w.name);
        std::fs::rename(&package, &source).map_err(io_err(&source))?;
        Ok(source)
    }

    /// Downloads a CRAN source tarball, preferring CRAN (so it can be checked against the
    /// lockfile's SHA-256), then CRAN's archive, then P3M's copy.
    fn download_cran_source(
        &self,
        w: &Wanted,
        date: &str,
        dir: &Path,
        transfers: &dyn Transfers,
    ) -> Result<(PathBuf, Option<String>), InstallError> {
        let file = format!("{}_{}.tar.gz", w.name, w.version);
        let cran = [
            format!("{}/src/contrib/{file}", self.cran),
            format!("{}/src/contrib/Archive/{}/{file}", self.cran, w.name),
        ];
        let path = dir.join(&file);
        for url in &cran {
            match self.http.get_tracked(url, &w.name, transfers) {
                Ok(bytes) => {
                    check_sha256(w, &bytes, w.sha256.as_deref())?;
                    std::fs::write(&path, &bytes).map_err(io_err(&path))?;
                    return Ok((path, Some(hex(&Sha256::digest(&bytes)))));
                }
                Err(HttpError::Status { status: 404, .. }) => continue,
                // CRAN unreachable (for example only an internal P3M is allowed): use P3M.
                Err(_) => break,
            }
        }
        match self.http.get_tracked(
            &self.p3m.source_url(date, &w.name, &w.version),
            &w.name,
            transfers,
        ) {
            Ok(bytes) => {
                std::fs::write(&path, &bytes).map_err(io_err(&path))?;
                Ok((path, None))
            }
            Err(HttpError::Status { status: 404, .. }) => Err(InstallError::SourceUnavailable {
                name: w.name.clone(),
                version: w.version.clone(),
                from: "CRAN or P3M".to_string(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Runs `R CMD INSTALL` for one package with only the given packages visible.
    fn build_one(
        &self,
        w: &Wanted,
        source: &Path,
        available: &HashMap<String, PathBuf>,
        git: bool,
    ) -> Result<PathBuf, InstallError> {
        let tmp = self.cache.temp_dir()?;
        let (deps, out, empty) = (
            tmp.path().join("deps"),
            tmp.path().join("out"),
            tmp.path().join("empty"),
        );
        for d in [&deps, &out, &empty] {
            std::fs::create_dir_all(d).map_err(io_err(d))?;
        }
        for (name, path) in available {
            let link = deps.join(name);
            fsutil::link_dir(path, &link).map_err(io_err(&link))?;
        }
        let logs = self.cache.root().join(".logs");
        std::fs::create_dir_all(&logs).map_err(io_err(&logs))?;
        let log = logs.join(format!("{}_{}.log", w.name, w.version));
        let log_file = std::fs::File::create(&log).map_err(io_err(&log))?;
        let log_err = log_file.try_clone().map_err(io_err(&log))?;
        let r = self
            .r
            .r_home
            .join("bin")
            .join(if cfg!(windows) { "R.exe" } else { "R" });
        // R_HOME of another R (rok started from R) would point this R elsewhere.
        let status = Command::new(&r)
            .env_remove("R_HOME")
            .args(["CMD", "INSTALL", "--no-docs", "--no-multiarch"])
            .arg(format!("--library={}", out.display()))
            .arg(source)
            .env("R_LIBS", &deps)
            .env("R_LIBS_USER", &empty)
            .env("R_LIBS_SITE", &empty)
            .env("R_PROFILE_USER", empty.join("none"))
            .env("R_ENVIRON_USER", empty.join("none"))
            .envs(&w.env)
            .stdout(log_file)
            .stderr(log_err)
            .status()
            .map_err(io_err(&r))?;
        if !status.success() {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            return Err(InstallError::Build(Box::new(BuildFailure {
                name: w.name.clone(),
                version: w.version.clone(),
                log,
                hint: build_hint(&text),
            })));
        }
        let key = if git {
            git_key(&self.build_key(w))
        } else {
            self.build_key(w)
        };
        Ok(self
            .cache
            .insert_dir(&w.name, &w.version, &key, &out.join(&w.name))?)
    }
}

/// Orders packages so that each comes after those of its dependencies that are also being built.
fn build_order<'a>(items: &[&'a Wanted]) -> Vec<&'a Wanted> {
    let names: HashSet<&str> = items.iter().map(|w| w.name.as_str()).collect();
    let mut done: HashSet<&str> = HashSet::new();
    let mut order = Vec::new();
    let mut rest: Vec<&Wanted> = items.to_vec();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    while !rest.is_empty() {
        let ready = rest.iter().position(|w| {
            w.dependencies
                .iter()
                .all(|d| !names.contains(d.as_str()) || done.contains(d.as_str()) || *d == w.name)
        });
        // A cycle cannot happen in valid CRAN metadata; build the rest in name order if it does.
        let w = rest.remove(ready.unwrap_or(0));
        done.insert(w.name.as_str());
        order.push(w);
    }
    order
}

/// The first line of a build log that hints at a missing tool or library (V3c).
fn build_hint(log: &str) -> Option<String> {
    log.lines()
        .map(str::trim)
        .find(|l| {
            l.contains(": not found")
                || l.contains("command not found")
                || l.contains("No such file or directory") && l.contains(".h")
                || l.contains("was not found in the pkg-config search path")
                || l.contains("cannot find -l")
        })
        .map(str::to_string)
}

/// Result of [`link`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LinkReport {
    /// Packages linked or re-linked.
    pub linked: Vec<String>,
    /// rok's links removed because the lockfile no longer has them.
    pub removed: Vec<String>,
}

/// Makes `library` contain exactly the given packages as links into the cache. Entries that
/// rok did not create (not links into `cache_root`) are left alone unless a wanted package
/// has the same name, in which case the lockfile wins. rok's links named in `keep` stay even
/// if they are not wanted (packages a partial sync left as they were).
pub fn link(
    library: &Path,
    packages: &[(String, PathBuf)],
    cache_root: &Path,
    keep: &HashSet<String>,
) -> Result<LinkReport, InstallError> {
    std::fs::create_dir_all(library).map_err(io_err(library))?;
    let (to_link, to_remove) = link_plan(library, packages, cache_root, keep)?;
    let mut report = LinkReport::default();
    for name in to_remove {
        let path = library.join(&name);
        fsutil::remove_link(&path).map_err(io_err(&path))?;
        report.removed.push(name);
    }
    for (name, target) in to_link {
        let path = library.join(&name);
        // Create the new link beside the old entry, then swap it in.
        let tmp = library.join(format!(".{name}.rok-tmp"));
        if fsutil::link_target(&tmp).is_some() {
            let _ = fsutil::remove_link(&tmp);
        }
        fsutil::link_dir(&target, &tmp).map_err(io_err(&tmp))?;
        if fsutil::link_target(&path).is_some() {
            // Windows cannot rename onto an existing directory, which a junction is.
            if cfg!(windows) {
                fsutil::remove_link(&path).map_err(io_err(&path))?;
            }
        } else if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(io_err(&path))?;
        }
        std::fs::rename(&tmp, &path).map_err(io_err(&path))?;
        report.linked.push(name);
    }
    Ok(report)
}

/// What [`link`] would change, without changing anything.
pub fn link_changes(
    library: &Path,
    packages: &[(String, PathBuf)],
    cache_root: &Path,
) -> Result<LinkReport, InstallError> {
    if !library.is_dir() {
        let mut linked: Vec<String> = packages.iter().map(|(n, _)| n.clone()).collect();
        linked.sort();
        return Ok(LinkReport {
            linked,
            removed: Vec::new(),
        });
    }
    let (to_link, removed) = link_plan(library, packages, cache_root, &HashSet::new())?;
    Ok(LinkReport {
        linked: to_link.into_iter().map(|(n, _)| n).collect(),
        removed,
    })
}

/// The links to create and the links to remove, both sorted by name.
type LinkPlan = (Vec<(String, PathBuf)>, Vec<String>);

fn link_plan(
    library: &Path,
    packages: &[(String, PathBuf)],
    cache_root: &Path,
    keep: &HashSet<String>,
) -> Result<LinkPlan, InstallError> {
    let wanted: HashMap<&str, &Path> = packages
        .iter()
        .map(|(n, p)| (n.as_str(), p.as_path()))
        .collect();
    let mut to_remove = Vec::new();
    for entry in std::fs::read_dir(library).map_err(io_err(library))? {
        let entry = entry.map_err(io_err(library))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if wanted.contains_key(name.as_str()) || name.starts_with('.') || keep.contains(&name) {
            continue;
        }
        let target = fsutil::link_target(&entry.path());
        if target.is_some_and(|t| t.starts_with(cache_root)) {
            to_remove.push(name);
        }
    }
    let mut to_link: Vec<(String, PathBuf)> = packages
        .iter()
        .filter(|(name, target)| {
            fsutil::link_target(&library.join(name)).as_deref() != Some(target.as_path())
        })
        .cloned()
        .collect();
    to_link.sort();
    to_remove.sort();
    Ok((to_link, to_remove))
}

/// Which build tools R needs are missing, without running R: on Windows the Rtools that
/// `etc/Rcmd_environ` names (V5), on macOS Apple's command line tools (V6), on Linux the tools
/// configured in `R_HOME/etc/Makeconf` (`CC`, `CXX`) and make (`R CMD config` needs make).
pub fn missing_build_tools(r: &RInstallation) -> Vec<String> {
    if let Some((name, dir)) = rtools(r) {
        return if dir.join("usr/bin/make.exe").is_file() {
            Vec::new()
        } else {
            vec![name]
        };
    }
    // On macOS, /usr/bin/clang and make exist even without the command line tools (they
    // offer to install them), so ask xcode-select whether the tools are there (V6).
    if cfg!(target_os = "macos") {
        let present = Command::new("xcode-select")
            .arg("-p")
            .output()
            .is_ok_and(|o| o.status.success());
        return if present {
            Vec::new()
        } else {
            vec!["Xcode command line tools".to_string()]
        };
    }
    let makeconf = std::fs::read_to_string(r.r_home.join("etc/Makeconf")).unwrap_or_default();
    let var = |key: &str| {
        makeconf.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| v.split_whitespace().next().unwrap_or("").to_string())
        })
    };
    let make = std::env::var("MAKE")
        .ok()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "make".to_string());
    [var("CC"), var("CXX"), Some(make)]
        .into_iter()
        .flatten()
        .filter(|cmd| !cmd.is_empty() && !on_path(cmd))
        .collect()
}

/// The Rtools a Windows R builds with: its name (`Rtools45`) and directory, from the line
/// `R_RTOOLS45_PATH="${RTOOLS45_HOME:-c:/rtools45}/..."` of `etc/Rcmd_environ`. `None` for
/// other R builds.
fn rtools(r: &RInstallation) -> Option<(String, PathBuf)> {
    let text = std::fs::read_to_string(r.r_home.join("etc/Rcmd_environ")).ok()?;
    let line = text
        .lines()
        .map(str::trim_start)
        .find(|l| l.starts_with("R_RTOOLS") && l.contains("_PATH="))?;
    let (var, rest) = line.split_once("${")?.1.split_once(":-")?;
    let default = rest.split('}').next()?;
    let number = var.strip_prefix("RTOOLS")?.strip_suffix("_HOME")?;
    let dir = std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string());
    Some((format!("Rtools{number}"), PathBuf::from(dir)))
}

/// How to get the build tools on an OS.
pub fn build_tools_advice(os: Os) -> &'static str {
    match os {
        Os::Windows => "Install Rtools from https://cran.r-project.org/bin/windows/Rtools/",
        Os::MacOs => "Install Apple's command line tools: xcode-select --install",
        Os::Linux => "On Debian or Ubuntu: sudo apt-get install -y build-essential",
    }
}

fn on_path(cmd: &str) -> bool {
    let p = Path::new(cmd);
    if p.is_absolute() {
        return p.is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(cmd).is_file()))
}

/// Unmanaged packages are installed by the user, never by rok.
fn unmanaged(w: &Wanted) -> InstallError {
    InstallError::SourceUnavailable {
        name: w.name.clone(),
        version: w.version.clone(),
        from: "rok: it is unmanaged, so install it yourself".to_string(),
    }
}

/// The cache key of a release rebuilt from its Git origin: kept apart from builds of the
/// repository's own tarball, which may differ.
fn git_key(build_key: &str) -> String {
    format!("{build_key}-git")
}

/// The end of the cache key of a package built from a GitHub commit.
pub fn github_key_suffix(commit: &str) -> String {
    format!("-gh-{}", &commit[..12.min(commit.len())])
}

/// Checks a download against the expected SHA-256, if there is one.
fn check_sha256(w: &Wanted, bytes: &[u8], expected: Option<&str>) -> Result<(), InstallError> {
    check_checksum(
        w,
        bytes,
        expected.map(|e| Checksum::Sha256(e.to_string())).as_ref(),
    )
}

/// Checks a download against the expected checksum, if there is one.
fn check_checksum(
    w: &Wanted,
    bytes: &[u8],
    expected: Option<&Checksum>,
) -> Result<(), InstallError> {
    let (algorithm, expected, actual) = match expected {
        None => return Ok(()),
        Some(Checksum::Sha256(e)) => ("SHA-256", e, hex(&Sha256::digest(bytes))),
        Some(Checksum::Md5(e)) => ("MD5", e, hex(&Md5::digest(bytes))),
    };
    if actual != expected.to_ascii_lowercase() {
        return Err(InstallError::ChecksumMismatch(Box::new(Mismatch {
            name: w.name.clone(),
            version: w.version.clone(),
            algorithm,
            expected: expected.clone(),
            actual,
        })));
    }
    Ok(())
}

/// The SHA-256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(name: &str, deps: &[&str]) -> Wanted {
        Wanted {
            name: name.into(),
            version: "1.0".parse().unwrap(),
            origin: Origin::Snapshot("2026-10-01".into()),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            sha256: None,
            env: BTreeMap::new(),
            remote: None,
        }
    }

    #[test]
    fn orders_builds_by_dependency() {
        let (a, b, c) = (
            wanted("a", &["b", "Rcpp"]),
            wanted("b", &["c"]),
            wanted("c", &[]),
        );
        let order: Vec<&str> = build_order(&[&a, &b, &c])
            .iter()
            .map(|w| w.name.as_str())
            .collect();
        assert_eq!(order, ["c", "b", "a"]);
    }

    #[test]
    fn reads_the_rtools_of_windows_builds_of_r() {
        let t = tempfile::tempdir().unwrap();
        let r = RInstallation {
            version: "4.6.1".parse().unwrap(),
            r_home: t.path().to_path_buf(),
            executable: t.path().join("bin/R.exe"),
            kind: crate::rdetect::RKind::Managed,
        };
        assert_eq!(rtools(&r), None, "not a Windows build");
        std::fs::create_dir_all(t.path().join("etc")).unwrap();
        std::fs::write(
            t.path().join("etc/Rcmd_environ"),
            "# INSTALLER-BUILD-aarch64:R_RTOOLS45_PATH=\"${RTOOLS45_AARCH64_HOME:-c:/rtools45-aarch64}/usr/bin\"\n\
             R_RTOOLS45_PATH=\"${RTOOLS99_HOME:-c:/no/rtools45}/x86_64-w64-mingw32.static.posix/bin;${RTOOLS45_HOME:-c:/rtools45}/usr/bin\"\n",
        )
        .unwrap();
        assert_eq!(
            rtools(&r),
            Some(("Rtools99".to_string(), PathBuf::from("c:/no/rtools45")))
        );
        assert_eq!(missing_build_tools(&r), ["Rtools99"]);
    }

    #[test]
    fn finds_build_hints() {
        let log = "* installing *source* package 'xml2' ...\nfoo.c:1:10: fatal error: libxml/xmlversion.h: No such file or directory\n";
        assert_eq!(
            build_hint(log).as_deref(),
            Some("foo.c:1:10: fatal error: libxml/xmlversion.h: No such file or directory")
        );
        assert_eq!(
            build_hint("sh: 1: make: not found").as_deref(),
            Some("sh: 1: make: not found")
        );
        assert_eq!(build_hint("* DONE (cli)"), None);
    }

    #[test]
    fn links_only_what_is_wanted_and_leaves_user_packages() {
        let t = tempfile::tempdir().unwrap();
        let cache = fsutil::canonicalize(t.path()).unwrap().join("cache");
        let lib = t.path().join("lib");
        let mk = |p: &Path| std::fs::create_dir_all(p).unwrap();
        let (a1, a2, b, old) = (
            cache.join("a/1"),
            cache.join("a/2"),
            cache.join("b/1"),
            cache.join("old/1"),
        );
        for p in [&a1, &a2, &b, &old] {
            mk(p);
        }
        mk(&lib.join("userpkg"));
        let none = HashSet::new();
        assert_eq!(
            link_changes(&lib, &[("a".into(), a1.clone())], &cache)
                .unwrap()
                .linked,
            ["a"]
        );
        let r = link(
            &lib,
            &[("a".into(), a1.clone()), ("old".into(), old.clone())],
            &cache,
            &none,
        )
        .unwrap();
        assert_eq!(r.linked, ["a", "old"]);
        // A partial sync keeps links it was told to keep.
        let kept = HashSet::from(["old".to_string()]);
        let r = link(&lib, &[("a".into(), a1.clone())], &cache, &kept).unwrap();
        assert_eq!(r, LinkReport::default());
        assert!(lib.join("old").exists());
        // Change a's version, add b, drop old; the user's own package stays.
        let wanted = [("a".to_string(), a2.clone()), ("b".to_string(), b.clone())];
        assert_eq!(
            link_changes(&lib, &wanted, &cache).unwrap(),
            LinkReport {
                linked: vec!["a".into(), "b".into()],
                removed: vec!["old".into()]
            }
        );
        let r = link(&lib, &wanted, &cache, &none).unwrap();
        assert_eq!(
            r,
            LinkReport {
                linked: vec!["a".into(), "b".into()],
                removed: vec!["old".into()]
            }
        );
        assert_eq!(fsutil::link_target(&lib.join("a")).unwrap(), a2);
        assert!(lib.join("userpkg").is_dir());
        // Nothing to do the second time.
        assert_eq!(
            link(&lib, &wanted, &cache, &none).unwrap(),
            LinkReport::default()
        );
        assert_eq!(
            link_changes(&lib, &wanted, &cache).unwrap(),
            LinkReport::default()
        );
    }

    #[test]
    fn source_keys_depend_on_env() {
        let empty = BTreeMap::new();
        let env = BTreeMap::from([("NOT_CRAN".to_string(), "true".to_string())]);
        let platform = Platform {
            os: crate::platform::Os::Linux,
            arch: Arch::X86_64,
            distro: Some(crate::platform::LinuxDistro::parse_os_release(
                "ID=ubuntu\nVERSION_ID=24.04\nUBUNTU_CODENAME=noble\n",
            )),
        };
        let r = RInstallation {
            version: "4.6.1".parse().unwrap(),
            r_home: PathBuf::from("/r"),
            executable: PathBuf::from("/r/bin/R"),
            kind: crate::rdetect::RKind::Managed,
        };
        let dirs = crate::paths::UserDirs::under(Path::new("/x"));
        let (http, cache) = (Http::new(), PackageCache::new(&dirs));
        let p3m = P3m::new(crate::p3m::DEFAULT_URL, http.clone(), &dirs);
        let ctx = Context {
            http: &http,
            p3m: &p3m,
            repos: Repositories::new(&http, &dirs),
            github: GitHub::new(&http),
            cache: &cache,
            platform: &platform,
            r: &r,
            cran: DEFAULT_CRAN,
            jobs: 1,
        };
        assert_eq!(ctx.binary_key().as_deref(), Some("4.6-noble"));
        let mut gh = wanted("a", &[]);
        gh.origin = Origin::GitHub {
            owner: "o".into(),
            repo: "a".into(),
            reference: crate::manifest::GitRef::DefaultBranch,
            commit: "0123456789abcdef".repeat(3)[..40].to_string(),
        };
        assert_eq!(
            ctx.build_key(&gh),
            "4.6-ubuntu-24.04-x86_64-source-gh-0123456789ab"
        );
        let mut from_repo = wanted("a", &[]);
        from_repo.origin = Origin::Repository {
            alias: "mv".into(),
            url: "https://x".into(),
        };
        assert!(
            ctx.build_key(&from_repo)
                .starts_with("4.6-ubuntu-24.04-x86_64-source-repo-")
        );
        assert!(
            ctx.repo_binary_key("https://x")
                .unwrap()
                .starts_with("4.6-noble-repo-")
        );
        assert_eq!(ctx.source_key(&empty), "4.6-ubuntu-24.04-x86_64-source");
        let with_env = ctx.source_key(&env);
        assert!(
            with_env.starts_with("4.6-ubuntu-24.04-x86_64-source-") && with_env.len() == 39,
            "{with_env}"
        );
    }
}
