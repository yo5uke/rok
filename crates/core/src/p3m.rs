//! Posit Package Manager (P3M): snapshot dates and package indexes.
//!
//! Facts used here were checked in phase 0 (V3, V10): `/__api__/repos/cran/transaction-dates`
//! lists the published snapshot dates; `/cran/<date>/src/contrib/PACKAGES.gz` is the index of a
//! date (the Linux binary index is identical); a dated index never changes, so it is cached
//! forever; the index lists R-devel's recommended packages a second time with a `Path` field.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::constraint::Constraint;
use crate::date;
use crate::dcf::{self, Dependency};
use crate::http::{Http, HttpError};
use crate::paths::UserDirs;
use crate::platform::{Arch, Os};
use crate::version::Version;

/// The public P3M instance.
pub const DEFAULT_URL: &str = "https://packagemanager.posit.co";

/// How long a downloaded list of snapshot dates is used before it is fetched again.
const DATES_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// How long P3M's information about a package is used before it is fetched again.
const PACKAGE_INFO_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The part of `/__api__/repos/cran/packages/<name>` that rok uses.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct PackageInfo {
    version: String,
    #[serde(default)]
    checksum: String,
    date_publication: Option<String>,
    #[serde(default, deserialize_with = "null_as_empty")]
    archived: Vec<ArchivedRelease>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct ArchivedRelease {
    /// Missing for some very old releases (nlme has entries with `"version": null`).
    version: Option<String>,
    date_publication: Option<String>,
}

fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ArchivedRelease>, D::Error> {
    Ok(Option::<Vec<ArchivedRelease>>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, thiserror::Error)]
pub enum P3mError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("unexpected response from {url}: {message}")]
    Response { url: String, message: String },
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> P3mError + '_ {
    move |source| P3mError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Why a snapshot date could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("`{0}` is not a date; use the form YYYY-MM-DD")]
    InvalidDate(String),
    #[error("{requested} is in the future (today is {today})")]
    InFuture { requested: String, today: String },
    #[error("{requested} is before the first P3M snapshot ({first})")]
    BeforeFirst { requested: String, first: String },
    #[error("P3M lists no snapshot dates")]
    NoSnapshots,
}

/// Resolves a snapshot date (requirements, chapter 7 "スナップショットの日付").
///
/// Without a request, returns the latest published date. A requested date resolves to the latest
/// published date on or before it. `dates` must be sorted. `today` is today's UTC date; one extra
/// day is allowed so that users ahead of UTC can ask for their local "today".
pub fn resolve_date(
    dates: &[String],
    requested: Option<&str>,
    today: &str,
) -> Result<String, SnapshotError> {
    let (Some(first), Some(last)) = (dates.first(), dates.last()) else {
        return Err(SnapshotError::NoSnapshots);
    };
    let Some(requested) = requested else {
        return Ok(last.clone());
    };
    if !date::is_valid(requested) {
        return Err(SnapshotError::InvalidDate(requested.to_string()));
    }
    let tomorrow = date::add_days(today, 1).unwrap_or_else(|| today.to_string());
    if requested > tomorrow.as_str() {
        return Err(SnapshotError::InFuture {
            requested: requested.to_string(),
            today: today.to_string(),
        });
    }
    if requested < first.as_str() {
        return Err(SnapshotError::BeforeFirst {
            requested: requested.to_string(),
            first: first.clone(),
        });
    }
    let i = dates.partition_point(|d| d.as_str() <= requested);
    Ok(dates[i - 1].clone())
}

/// The published snapshot dates, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDates {
    pub dates: Vec<String>,
    /// True when the list could not be refreshed and an older cached copy was used.
    pub stale: bool,
}

/// A P3M instance with a local cache.
#[derive(Debug, Clone)]
pub struct P3m {
    base: String,
    http: Http,
    cache: PathBuf,
}

impl P3m {
    /// `base` is the server URL, such as [`DEFAULT_URL`].
    pub fn new(base: &str, http: Http, dirs: &UserDirs) -> P3m {
        let base = base.trim_end_matches('/').to_string();
        let host: String = base
            .split_once("://")
            .map_or(base.as_str(), |(_, rest)| rest)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        P3m {
            cache: dirs.p3m_cache().join(host),
            base,
            http,
        }
    }

    /// This server's cache directory.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    /// The base of dated CRAN snapshots (`<server>/cran`), as written to lockfiles.
    pub fn cran_base(&self) -> String {
        format!("{}/cran", self.base)
    }

    /// The CRAN repository URL for a snapshot date, as written to lockfiles.
    pub fn cran_url(&self, date: &str) -> String {
        format!("{}/cran/{date}", self.base)
    }

    /// The URL of a package for a Linux distribution (`noble`, ...). P3M serves a binary for
    /// the R version in the User-Agent when it has one, and the source otherwise (V3).
    pub fn linux_package_url(
        &self,
        distro: &str,
        date: &str,
        name: &str,
        version: &Version,
    ) -> String {
        format!(
            "{}/cran/__linux__/{distro}/{date}/src/contrib/{name}_{version}.tar.gz",
            self.base
        )
    }

    /// The URL of a Windows or macOS binary, as listed in [`P3m::binary_index`].
    pub fn binary_url(&self, date: &str, dir: &BinaryDir, name: &str, version: &Version) -> String {
        format!(
            "{}/{}/{name}_{version}.{}",
            self.cran_url(date),
            dir.path,
            dir.ext
        )
    }

    /// The URL of a package's source as P3M serves it (with `Repository: RSPM` added, so it
    /// is not byte-identical to CRAN's file).
    pub fn source_url(&self, date: &str, name: &str, version: &Version) -> String {
        format!(
            "{}/src/contrib/{name}_{version}.tar.gz",
            self.cran_url(date)
        )
    }

    /// The SHA-256 of CRAN's source tarball of `name` `version`, if P3M knows it. P3M's package
    /// API returns the checksum of the current CRAN version only (V3), so older versions give
    /// `None`. Known checksums are cached forever.
    pub fn source_checksum(
        &self,
        name: &str,
        version: &Version,
    ) -> Result<Option<String>, P3mError> {
        let path = self
            .cache
            .join("checksums")
            .join(name)
            .join(version.as_str());
        if let Ok(h) = std::fs::read_to_string(&path) {
            return Ok(Some(h.trim().to_string()));
        }
        let Some(info) = self.package_info(name)? else {
            return Ok(None);
        };
        let valid =
            info.checksum.len() == 64 && info.checksum.bytes().all(|b| b.is_ascii_hexdigit());
        if info.version.parse::<Version>().ok().as_ref() != Some(version) || !valid {
            return Ok(None);
        }
        write_atomic(&path, info.checksum.as_bytes())?;
        Ok(Some(info.checksum))
    }

    /// P3M's system requirements (`-dev` packages and tools to install with the distribution's
    /// package manager) of `names` and of their dependencies, for a distribution such as
    /// (`ubuntu`, `24.04`). Packages without requirements are left out. Answers are cached for
    /// a day, so resolving again does not wait for the network.
    pub fn sysreqs(
        &self,
        names: &[&str],
        distribution: &str,
        release: &str,
    ) -> Result<BTreeMap<String, Vec<String>>, P3mError> {
        #[derive(Deserialize)]
        struct Response {
            requirements: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            name: String,
            requirements: Requirements,
        }
        #[derive(Deserialize)]
        struct Requirements {
            #[serde(default)]
            packages: Vec<String>,
        }
        /// A cached answer: when it was fetched (seconds since the epoch) and the packages.
        type Cached = BTreeMap<String, (u64, Vec<String>)>;

        let path = self
            .cache
            .join("sysreqs")
            .join(format!("{distribution}-{release}.json"));
        let mut cached: Cached = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let fresh = |at: u64| now.saturating_sub(at) < PACKAGE_INFO_MAX_AGE.as_secs();
        let stale: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| !cached.get(*n).is_some_and(|(at, _)| fresh(*at)))
            .collect();
        // Keep URLs short: P3M accepts many names per request, proxies may not.
        for chunk in stale.chunks(100) {
            let mut url = format!(
                "{}/__api__/repos/cran/sysreqs?all=false&distribution={distribution}&release={release}",
                self.base
            );
            for n in chunk {
                url.push_str("&pkgname=");
                url.push_str(n);
            }
            let body = self.http.get_bytes(&url, None)?;
            let r: Response = serde_json::from_slice(&body).map_err(|e| P3mError::Response {
                url: url.clone(),
                message: e.to_string(),
            })?;
            // The answer also lists dependencies; packages asked about but absent need nothing.
            for n in chunk {
                cached.insert(n.to_string(), (now, Vec::new()));
            }
            for e in r.requirements {
                cached.insert(e.name, (now, e.requirements.packages));
            }
        }
        if !stale.is_empty() {
            write_atomic(&path, &serde_json::to_vec(&cached).expect("serializable"))?;
        }
        Ok(names
            .iter()
            .filter_map(|n| {
                let (_, packages) = cached.get(*n)?;
                (!packages.is_empty()).then(|| (n.to_string(), packages.clone()))
            })
            .collect())
    }

    /// The CRAN releases of a package with their publication times (UTC, RFC 3339), oldest
    /// first. Releases without a publication time are left out (V10).
    pub fn history(&self, name: &str) -> Result<Vec<(Version, String)>, P3mError> {
        let Some(info) = self.package_info(name)? else {
            return Ok(Vec::new());
        };
        let mut out: Vec<(Version, String)> =
            std::iter::once((Some(info.version), info.date_publication))
                .chain(
                    info.archived
                        .into_iter()
                        .map(|a| (a.version, a.date_publication)),
                )
                .filter_map(|(v, d)| Some((v?.parse().ok()?, d?)))
                .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        Ok(out)
    }

    /// P3M's information about a package (current version, its checksum, and the archived
    /// releases), cached for a day. `None` if P3M does not know the package. If P3M cannot be
    /// reached, an older cached copy is used.
    fn package_info(&self, name: &str) -> Result<Option<PackageInfo>, P3mError> {
        let path = self.cache.join("packages").join(format!("{name}.json"));
        let cached = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<PackageInfo>(&b).ok());
        let age = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        if let (Some(info), Some(age)) = (&cached, age)
            && age < PACKAGE_INFO_MAX_AGE
        {
            return Ok(Some(info.clone()));
        }
        let url = format!("{}/__api__/repos/cran/packages/{name}", self.base);
        let body = match self.http.get_bytes(&url, None) {
            Ok(b) => b,
            Err(HttpError::Status { status: 404, .. }) => return Ok(None),
            Err(e) => return cached.map(Some).ok_or(e.into()),
        };
        let info: PackageInfo = serde_json::from_slice(&body).map_err(|e| P3mError::Response {
            url: url.clone(),
            message: e.to_string(),
        })?;
        // Keep only the fields rok uses; the full response lists every reverse dependency.
        let slim = serde_json::to_vec(&info).expect("serializable");
        write_atomic(&path, &slim)?;
        Ok(Some(info))
    }

    /// The published snapshot dates. A cached list is used when it is less than an hour old, or
    /// when it already covers `needed` (a date that must not be newer than the last listed one).
    /// If the download fails, an older cached list is used and marked `stale`.
    pub fn snapshot_dates(&self, needed: Option<&str>) -> Result<SnapshotDates, P3mError> {
        let path = self.cache.join("transaction-dates.json");
        let cached = read_dates(&path);
        if let Some((dates, age)) = &cached {
            let covers =
                needed.is_some_and(|d| dates.last().is_some_and(|last| d <= last.as_str()));
            if age < &DATES_MAX_AGE || covers {
                return Ok(SnapshotDates {
                    dates: dates.clone(),
                    stale: false,
                });
            }
        }
        let url = format!("{}/__api__/repos/cran/transaction-dates", self.base);
        let body = match self.http.get_bytes(&url, None) {
            Ok(body) => body,
            Err(e) => {
                return match cached {
                    Some((dates, _)) => Ok(SnapshotDates { dates, stale: true }),
                    None => Err(e.into()),
                };
            }
        };
        let dates = parse_dates(&body).map_err(|message| P3mError::Response {
            url: url.clone(),
            message,
        })?;
        write_atomic(&path, &body)?;
        Ok(SnapshotDates {
            dates,
            stale: false,
        })
    }

    /// Resolves a snapshot date against the published dates (see [`resolve_date`]).
    pub fn resolve_snapshot(
        &self,
        requested: Option<&str>,
    ) -> Result<(String, SnapshotDates), P3mError> {
        let dates = self.snapshot_dates(requested)?;
        let resolved = resolve_date(&dates.dates, requested, &date::today_utc())?;
        Ok((resolved, dates))
    }

    /// The package index of a snapshot date, downloaded once and then read from the cache.
    pub fn index(&self, date: &str) -> Result<Index, P3mError> {
        if !date::is_valid(date) {
            return Err(SnapshotError::InvalidDate(date.to_string()).into());
        }
        let path = self.cache.join("cran").join(date).join("PACKAGES.gz");
        let gz = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let url = format!("{}/src/contrib/PACKAGES.gz", self.cran_url(date));
                let bytes = self.http.get_bytes(&url, None)?;
                // Check that the download is a readable index before caching it forever.
                let text = gunzip(&bytes).map_err(|message| P3mError::Response { url, message })?;
                write_atomic(&path, &bytes)?;
                return Ok(Index::parse(date, &text));
            }
            Err(e) => return Err(io_err(&path)(e)),
        };
        let text = gunzip(&gz).map_err(|message| P3mError::Response {
            url: path.display().to_string(),
            message,
        })?;
        Ok(Index::parse(date, &text))
    }
}

/// Where a snapshot keeps Windows or macOS binaries for one R minor version and CPU: the
/// path under the snapshot (`bin/windows/contrib/4.6`, `bin/macosx/sonoma-arm64/contrib/4.6`)
/// and the files' extension. Linux binaries come through the source URLs instead (V3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryDir {
    pub path: String,
    pub ext: &'static str,
}

/// How long P3M's list of macOS binary directories is reused.
const STATUS_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

impl P3m {
    /// Where P3M keeps binaries for R `r_minor` on this OS and CPU; `None` on Linux (see
    /// [`P3m::linux_package_url`]) and where P3M builds none (Windows on ARM, old R on macOS).
    pub fn binary_dir(
        &self,
        os: Os,
        arch: Arch,
        r_minor: &str,
    ) -> Result<Option<BinaryDir>, P3mError> {
        Ok(match (os, arch) {
            (Os::Windows, Arch::X86_64) => Some(BinaryDir {
                path: format!("bin/windows/contrib/{r_minor}"),
                ext: "zip",
            }),
            (Os::MacOs, arch) => self.macos_name(r_minor, arch)?.map(|name| BinaryDir {
                // An empty name means the plain directory (x86_64 for R 4.0 to 4.2).
                path: if name.is_empty() {
                    format!("bin/macosx/contrib/{r_minor}")
                } else {
                    format!("bin/macosx/{name}/contrib/{r_minor}")
                },
                ext: "tgz",
            }),
            _ => None,
        })
    }

    /// P3M's name for the macOS binaries of R `r_minor` on a CPU (`sonoma-arm64`,
    /// `big-sur-x86_64`), from `macos_urls` in `/__api__/status` (cached for a day).
    fn macos_name(&self, r_minor: &str, arch: Arch) -> Result<Option<String>, P3mError> {
        let path = self.cache.join("status.json");
        let age = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok());
        let cached = std::fs::read(&path).ok();
        let url = format!("{}/__api__/status", self.base);
        let body = match (&cached, age) {
            (Some(b), Some(age)) if age < STATUS_MAX_AGE => b.clone(),
            _ => match self.http.get_bytes(&url, None) {
                Ok(b) => {
                    write_atomic(&path, &b)?;
                    b
                }
                Err(e) => cached.ok_or(e)?,
            },
        };
        #[derive(Deserialize)]
        struct Status {
            #[serde(default)]
            macos_urls: HashMap<String, HashMap<String, String>>,
        }
        let status: Status = serde_json::from_slice(&body).map_err(|e| P3mError::Response {
            url,
            message: e.to_string(),
        })?;
        let arch = match arch {
            Arch::Aarch64 => "arm64",
            Arch::X86_64 => "x86_64",
        };
        Ok(status
            .macos_urls
            .get(r_minor)
            .and_then(|m| m.get(arch))
            .cloned())
    }

    /// The Windows or macOS binaries of a snapshot date in `dir`, with each file's MD5
    /// (`Hash`; V2). Downloaded once and then read from the cache. Empty if P3M has none.
    pub fn binary_index(&self, date: &str, dir: &BinaryDir) -> Result<Index, P3mError> {
        if !date::is_valid(date) {
            return Err(SnapshotError::InvalidDate(date.to_string()).into());
        }
        let path = self
            .cache
            .join("cran")
            .join(date)
            .join(dir.path.replace('/', "_"))
            .join("PACKAGES.gz");
        let label = format!("{date} ({})", dir.path);
        if let Ok(gz) = std::fs::read(&path) {
            let text = gunzip(&gz).map_err(|message| P3mError::Response {
                url: path.display().to_string(),
                message,
            })?;
            return Ok(Index::parse(&label, &text));
        }
        let url = format!("{}/{}/PACKAGES.gz", self.cran_url(date), dir.path);
        let bytes = match self.http.get_bytes(&url, None) {
            Ok(b) => b,
            Err(HttpError::Status { status: 404, .. }) => return Ok(Index::parse(&label, "")),
            Err(e) => return Err(e.into()),
        };
        let text = gunzip(&bytes).map_err(|message| P3mError::Response { url, message })?;
        write_atomic(&path, &bytes)?;
        Ok(Index::parse(&label, &text))
    }
}

fn read_dates(path: &Path) -> Option<(Vec<String>, Duration)> {
    let body = std::fs::read(path).ok()?;
    let age = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .unwrap_or(Duration::MAX);
    Some((parse_dates(&body).ok()?, age))
}

fn parse_dates(body: &[u8]) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Entry {
        alias: String,
    }
    let entries: Vec<Entry> = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let mut dates: Vec<String> = entries
        .into_iter()
        .map(|e| e.alias)
        .filter(|d| date::is_valid(d))
        .collect();
    dates.sort();
    dates.dedup();
    if dates.is_empty() {
        return Err("no snapshot dates".to_string());
    }
    Ok(dates)
}

fn gunzip(bytes: &[u8]) -> Result<String, String> {
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut text)
        .map_err(|e| format!("not a gzip-compressed text file ({e})"))?;
    Ok(text)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), P3mError> {
    crate::fsutil::write_atomic(path, bytes).map_err(io_err(path))
}

/// One package in an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: String,
    pub version: Version,
    /// The R versions the package supports, from `Depends: R (...)`.
    pub r_constraint: Constraint,
    /// `Depends` (without R) and `Imports`: needed to load the package.
    pub dependencies: Vec<Dependency>,
    /// `LinkingTo`: needed only to build the package from source.
    pub linking_to: Vec<Dependency>,
    pub needs_compilation: bool,
    /// `OS_type` (`unix` or `windows`), when the package is limited to one.
    pub os_type: Option<String>,
    /// `SHA256` of the file, in indexes that have it (R-multiverse, r-universe).
    pub sha256: Option<String>,
    /// MD5 of the file: `Hash` in P3M's Windows and macOS indexes, `MD5sum` in CRAN's.
    pub md5: Option<String>,
    /// `Path`: where the file is, relative to the index (r-universe uses it for every package).
    pub path: Option<String>,
    /// Whether the index lists a binary (it has a `Built` field).
    pub built: bool,
}

/// A snapshot's package index.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub date: String,
    entries: HashMap<String, IndexEntry>,
    /// Records that could not be read: (package, reason).
    pub skipped: Vec<(String, String)>,
}

impl Index {
    /// Parses a P3M PACKAGES text. Records with a `Path` field (R-devel's recommended packages)
    /// are ignored; unreadable records are listed in `skipped`.
    pub fn parse(date: &str, text: &str) -> Index {
        Index::parse_with(date, text, false)
    }

    /// Parses the PACKAGES text of another CRAN-like repository, keeping `Path` records.
    pub fn parse_repository(label: &str, text: &str) -> Index {
        Index::parse_with(label, text, true)
    }

    fn parse_with(date: &str, text: &str, keep_path: bool) -> Index {
        let mut index = Index {
            date: date.to_string(),
            ..Default::default()
        };
        let records = match dcf::parse(text) {
            Ok(r) => r,
            Err(e) => {
                index.skipped.push(("PACKAGES".to_string(), e.to_string()));
                return index;
            }
        };
        for rec in records {
            if !keep_path && rec.get("Path").is_some() {
                continue;
            }
            let name = rec.get("Package").unwrap_or("").to_string();
            match entry_from_record(&rec) {
                Ok(entry) => {
                    let keep_existing = index
                        .entries
                        .get(&name)
                        .is_some_and(|e| e.version >= entry.version);
                    if !keep_existing {
                        index.entries.insert(name, entry);
                    }
                }
                Err(reason) => index.skipped.push((name, reason)),
            }
        }
        index
    }

    pub fn get(&self, name: &str) -> Option<&IndexEntry> {
        self.entries.get(name)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &IndexEntry> {
        self.entries.values()
    }
}

fn entry_from_record(rec: &dcf::Record) -> Result<IndexEntry, String> {
    let name = rec.get("Package").ok_or("no Package field")?;
    if !dcf::is_package_name(name) {
        return Err(format!("invalid package name `{name}`"));
    }
    let version = rec
        .get("Version")
        .ok_or("no Version field")?
        .parse::<Version>()
        .map_err(|e| e.to_string())?;
    let field = |name: &str| -> Result<Vec<Dependency>, String> {
        rec.get(name)
            .map(|v| dcf::parse_dependencies(v).map_err(|e| format!("{name}: {e}")))
            .transpose()
            .map(Option::unwrap_or_default)
    };
    let mut r_constraint = Constraint::any();
    let mut dependencies = Vec::new();
    for dep in field("Depends")?.into_iter().chain(field("Imports")?) {
        if dep.name == "R" {
            r_constraint = r_constraint.and(&dep.constraint);
        } else {
            dependencies.push(dep);
        }
    }
    let linking_to = field("LinkingTo")?;
    Ok(IndexEntry {
        name: name.to_string(),
        version,
        r_constraint,
        dependencies,
        linking_to,
        needs_compilation: rec.get("NeedsCompilation") == Some("yes"),
        os_type: rec.get("OS_type").map(str::to_string),
        sha256: rec
            .get("SHA256")
            .filter(|h| h.len() == 64)
            .map(str::to_ascii_lowercase),
        md5: rec
            .get("Hash")
            .or_else(|| rec.get("MD5sum"))
            .filter(|h| h.len() == 32)
            .map(str::to_ascii_lowercase),
        path: rec.get("Path").map(str::to_string),
        built: rec.get("Built").is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_package_info_with_missing_archived_versions() {
        let json = r#"{"version": "3.1-171", "checksum": "ab", "date_publication": "2026-09-01T09:55:28Z",
            "archived": [{"version": null, "date_publication": null}, {"version": "3.1-170", "date_publication": "2026-07-15T12:14:44Z"}]}"#;
        let info: PackageInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.archived.len(), 2);
        assert_eq!(info.archived[0].version, None);
    }

    fn dates(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolves_snapshot_dates() {
        let d = dates(&["2017-10-10", "2024-01-09", "2024-01-12", "2026-10-05"]);
        let today = "2026-10-06";
        assert_eq!(resolve_date(&d, None, today).unwrap(), "2026-10-05");
        assert_eq!(
            resolve_date(&d, Some("2024-01-10"), today).unwrap(),
            "2024-01-09"
        );
        assert_eq!(
            resolve_date(&d, Some("2024-01-12"), today).unwrap(),
            "2024-01-12"
        );
        assert_eq!(
            resolve_date(&d, Some("2026-10-06"), today).unwrap(),
            "2026-10-05"
        );
        // One day ahead of UTC is allowed (users east of UTC), two days is the future.
        assert_eq!(
            resolve_date(&d, Some("2026-10-07"), today).unwrap(),
            "2026-10-05"
        );
        assert!(matches!(
            resolve_date(&d, Some("2026-10-08"), today),
            Err(SnapshotError::InFuture { .. })
        ));
        assert!(matches!(
            resolve_date(&d, Some("2017-01-01"), today),
            Err(SnapshotError::BeforeFirst { .. })
        ));
        assert!(matches!(
            resolve_date(&d, Some("2026-02-30"), today),
            Err(SnapshotError::InvalidDate(_))
        ));
        assert_eq!(
            resolve_date(&[], None, today),
            Err(SnapshotError::NoSnapshots)
        );
    }

    #[test]
    fn parses_transaction_dates() {
        let body = br#"[{"repo_id":2,"alias":"2026-10-05","date":"2026-10-05T00:00:00Z"},{"alias":"2017-10-10"},{"alias":"latest"}]"#;
        assert_eq!(parse_dates(body).unwrap(), ["2017-10-10", "2026-10-05"]);
        assert!(parse_dates(b"[]").is_err());
        assert!(parse_dates(b"<html>").is_err());
    }

    const PACKAGES: &str = "\
Package: fixest
Version: 0.14.2
Depends: R (>= 3.5.0)
Imports: stats, Rcpp (>= 1.0.5), dreamerr (>= 1.4.0)
LinkingTo: Rcpp
NeedsCompilation: yes
License: GPL-3

Package: boot
Version: 1.3-31
Priority: recommended
Depends: R (>= 3.0.0), graphics, stats
NeedsCompilation: no

Package: boot
Version: 1.3-32
Priority: recommended
Path: 4.7.0/Recommended
NeedsCompilation: no

Package: winonly
Version: 1.0
OS_type: windows

Package: broken
Version: one
";

    #[test]
    fn parses_an_index() {
        let index = Index::parse("2026-10-01", PACKAGES);
        assert_eq!(index.len(), 3);
        let fixest = index.get("fixest").unwrap();
        assert_eq!(fixest.version.as_str(), "0.14.2");
        assert_eq!(fixest.r_constraint.to_string(), ">= 3.5.0");
        let deps: Vec<_> = fixest
            .dependencies
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(deps, ["stats", "Rcpp", "dreamerr"]);
        assert_eq!(fixest.linking_to[0].name, "Rcpp");
        assert!(fixest.needs_compilation);
        // The R-devel copy of a recommended package (with `Path`) is ignored.
        assert_eq!(index.get("boot").unwrap().version.as_str(), "1.3-31");
        assert_eq!(
            index.get("winonly").unwrap().os_type.as_deref(),
            Some("windows")
        );
        assert_eq!(index.skipped.len(), 1);
        assert_eq!(index.skipped[0].0, "broken");
    }

    #[test]
    fn reads_index_from_cache_without_network() {
        let t = tempfile::tempdir().unwrap();
        let dirs = UserDirs::under(t.path());
        // An unreachable server: only the cache can answer.
        let p3m = P3m::new("http://127.0.0.1:9", Http::new(), &dirs);
        let path = dirs
            .p3m_cache()
            .join("127.0.0.1_9/cran/2026-10-01/PACKAGES.gz");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, PACKAGES.as_bytes()).unwrap();
        std::fs::write(&path, gz.finish().unwrap()).unwrap();
        let index = p3m.index("2026-10-01").unwrap();
        assert_eq!(index.get("fixest").unwrap().version.as_str(), "0.14.2");
        assert!(matches!(p3m.index("2026-10-02"), Err(P3mError::Http(_))));
        assert!(matches!(
            p3m.index("not-a-date"),
            Err(P3mError::Snapshot(_))
        ));
        assert_eq!(
            p3m.cran_url("2026-10-01"),
            "http://127.0.0.1:9/cran/2026-10-01"
        );
    }

    #[test]
    fn falls_back_to_cached_dates_when_offline() {
        let t = tempfile::tempdir().unwrap();
        let dirs = UserDirs::under(t.path());
        let p3m = P3m::new("http://127.0.0.1:9/", Http::new(), &dirs);
        assert!(p3m.snapshot_dates(None).is_err());
        let path = dirs.p3m_cache().join("127.0.0.1_9/transaction-dates.json");
        write_atomic(&path, br#"[{"alias":"2026-10-01"},{"alias":"2026-10-02"}]"#).unwrap();
        // Fresh cache: used without a request.
        let fresh = p3m.snapshot_dates(None).unwrap();
        assert_eq!((fresh.dates.len(), fresh.stale), (2, false));
        // A date the cache covers is resolved from the cache.
        assert_eq!(
            p3m.resolve_snapshot(Some("2026-10-01")).unwrap().0,
            "2026-10-01"
        );
    }
}
