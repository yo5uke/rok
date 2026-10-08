//! renv.lock (requirements chapter 7): reading it to migrate a project from renv, and writing
//! it from rok.lock.
//!
//! The renv.lock that rok writes is minimal (V9): no `Hash`, P3M's dated source URLs as
//! repositories (renv turns them into Linux binary URLs itself), and each package names its
//! repository, so packages from different snapshot dates restore from the right one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::dcf::parse_dependencies;
use crate::lockfile::{Lockfile, Remote, Source};
use crate::manifest::GitRef;
use crate::version::Version;

/// File name of renv's lockfile.
pub const FILE_NAME: &str = "renv.lock";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenvError {
    #[error("renv.lock is not valid JSON: {0}")]
    Json(String),
    #[error("renv.lock: {0}")]
    Invalid(String),
}

/// A renv.lock, as far as rok uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenvLock {
    /// The R version renv recorded.
    pub r: Version,
    /// `R.Repositories`: (name, URL).
    pub repositories: Vec<(String, String)>,
    pub packages: Vec<RenvPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenvPackage {
    pub name: String,
    pub version: Version,
    pub source: RenvSource,
    /// Packages it needs (Depends, Imports, LinkingTo, or renv's `Requirements`), without R and
    /// base packages.
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenvSource {
    /// A CRAN-like repository: `repository` as renv wrote it (a name from `R.Repositories`, or
    /// a URL). `remote` is the Git origin r-universe records.
    Repository {
        repository: Option<String>,
        remote: Option<Remote>,
    },
    GitHub {
        owner: String,
        repo: String,
        /// `RemoteRef` (`HEAD` for the default branch).
        reference: Option<String>,
        commit: String,
    },
    /// A source rok cannot use yet (`Bioconductor`, `Local`, `URL`, `GitLab`, ...).
    Other(String),
}

fn text<'a>(v: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str().filter(|s| !s.is_empty())
}

/// The package names in a DESCRIPTION-style field that renv copied (an array of
/// `pkg (>= 1.0)` strings, or one comma-separated string).
fn field_names(v: Option<&Value>) -> Vec<String> {
    let joined = match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(s)) => s.clone(),
        _ => return Vec::new(),
    };
    parse_dependencies(&joined)
        .map(|deps| deps.into_iter().map(|d| d.name).collect())
        .unwrap_or_default()
}

impl RenvLock {
    pub fn parse(text_: &str) -> Result<RenvLock, RenvError> {
        let root: Value =
            serde_json::from_str(text_).map_err(|e| RenvError::Json(e.to_string()))?;
        let r = root
            .pointer("/R/Version")
            .and_then(Value::as_str)
            .ok_or_else(|| RenvError::Invalid("no R version (`R.Version`)".into()))?;
        let r: Version = r
            .parse()
            .map_err(|_| RenvError::Invalid(format!("`{r}` is not an R version")))?;
        let repositories = root
            .pointer("/R/Repositories")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some((
                            x.get("Name")?.as_str()?.to_string(),
                            x.get("URL")?.as_str()?.trim_end_matches('/').to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut packages = Vec::new();
        let entries = root
            .get("Packages")
            .and_then(Value::as_object)
            .ok_or_else(|| RenvError::Invalid("no `Packages`".into()))?;
        for (key, entry) in entries {
            let entry = entry
                .as_object()
                .ok_or_else(|| RenvError::Invalid(format!("`{key}` is not an object")))?;
            let name = text(entry, "Package").unwrap_or(key).to_string();
            let version: Version = text(entry, "Version")
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| RenvError::Invalid(format!("`{name}` has no valid `Version`")))?;
            let remote = || {
                Some(Remote {
                    url: text(entry, "RemoteUrl")?.to_string(),
                    sha: text(entry, "RemoteSha")?.to_string(),
                    subdir: text(entry, "RemoteSubdir").map(str::to_string),
                    rebuilt: false,
                })
            };
            let source = match text(entry, "Source").unwrap_or("Repository") {
                // Older renv wrote `CRAN` for packages from CRAN.
                "Repository" | "CRAN" => RenvSource::Repository {
                    repository: text(entry, "Repository").map(str::to_string),
                    remote: remote(),
                },
                "GitHub" => RenvSource::GitHub {
                    owner: text(entry, "RemoteUsername")
                        .ok_or_else(|| {
                            RenvError::Invalid(format!("`{name}` has no `RemoteUsername`"))
                        })?
                        .to_string(),
                    repo: text(entry, "RemoteRepo")
                        .ok_or_else(|| RenvError::Invalid(format!("`{name}` has no `RemoteRepo`")))?
                        .to_string(),
                    reference: text(entry, "RemoteRef").map(str::to_string),
                    commit: text(entry, "RemoteSha")
                        .ok_or_else(|| RenvError::Invalid(format!("`{name}` has no `RemoteSha`")))?
                        .to_string(),
                },
                other => RenvSource::Other(other.to_string()),
            };
            let mut deps: BTreeSet<String> = match entry.get("Requirements") {
                Some(r) => field_names(Some(r)).into_iter().collect(),
                None => ["Depends", "Imports", "LinkingTo"]
                    .iter()
                    .flat_map(|k| field_names(entry.get(*k)))
                    .collect(),
            };
            deps.retain(|d| d != "R" && !crate::rpkgs::is_base(d));
            let mut dependencies: Vec<String> = deps.into_iter().collect();
            dependencies.sort_by(|a, b| crate::lockfile::name_order(a, b));
            packages.push(RenvPackage {
                name,
                version,
                source,
                dependencies,
            });
        }
        Ok(RenvLock {
            r,
            repositories,
            packages,
        })
    }

    /// The URL of the repository a package came from: its `Repository` (a name from
    /// `R.Repositories`, or a URL), or the first repository.
    pub fn repository_url(&self, repository: Option<&str>) -> Option<String> {
        match repository {
            Some(r) if r.contains("://") => Some(r.trim_end_matches('/').to_string()),
            Some(r) => self
                .repositories
                .iter()
                .find(|(n, _)| n == r)
                .map(|(_, u)| u.clone()),
            None => self.repositories.first().map(|(_, u)| u.clone()),
        }
    }

    /// Packages that no other package in the lockfile needs: the candidates to declare.
    pub fn roots(&self) -> Vec<String> {
        let needed: BTreeSet<&str> = self
            .packages
            .iter()
            .flat_map(|p| p.dependencies.iter().map(String::as_str))
            .collect();
        self.packages
            .iter()
            .filter(|p| p.name != "renv" && !needed.contains(p.name.as_str()))
            .map(|p| p.name.clone())
            .collect()
    }
}

/// Whether a repository URL is CRAN (a CRAN mirror, or CRAN on P3M), as opposed to another
/// CRAN-like repository such as r-universe.
pub fn is_cran(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    host.contains("cran")
        || host == "r-project.org"
        || host.ends_with(".r-project.org")
        || path.split('/').next() == Some("cran")
}

/// The snapshot date in a P3M URL (`.../cran/2024-06-03`, `.../__linux__/noble/2024-06-03`).
pub fn date_in_url(url: &str) -> Option<String> {
    url.split('/')
        .find(|seg| crate::date::is_valid(seg))
        .map(str::to_string)
}

/// The name of a CRAN-like repository for `[repositories]`: renv's name for it, or its host.
pub fn repository_alias(lock: &RenvLock, url: &str) -> String {
    if let Some((name, _)) = lock.repositories.iter().find(|(_, u)| u == url) {
        return name.clone();
    }
    let host = url
        .split_once("://")
        .map_or(url, |(_, r)| r)
        .split('/')
        .next()
        .unwrap_or("repo");
    host.split('.').next().unwrap_or("repo").to_string()
}

/// For each package, the snapshot dates (indices into `dates`) at which its version was the
/// current one: `start..end`. `None` for a version that no snapshot has.
pub type Interval = Option<(usize, usize)>;

/// The interval of `version` from a package's releases mapped to snapshot dates
/// (`resolve::release_snapshots` over all dates), in publication order.
pub fn interval(releases: &[(Version, String)], version: &Version, dates: &[String]) -> Interval {
    let at = releases.iter().position(|(v, _)| v == version)?;
    let start = dates.partition_point(|d| d < &releases[at].1);
    let end = releases
        .get(at + 1)
        .map_or(dates.len(), |(_, next)| dates.partition_point(|d| d < next));
    (start < end).then_some((start, end))
}

/// The snapshot date to use (V10): the latest date at which every version was current, or
/// else the latest of the dates at which the most were. Returns its index and how many
/// versions it matches.
pub fn best_date(intervals: &[(usize, usize)], dates: usize) -> Option<(usize, usize)> {
    if intervals.is_empty() || dates == 0 {
        return None;
    }
    let lo = intervals.iter().map(|i| i.0).max()?;
    let hi = intervals.iter().map(|i| i.1).min()?;
    if lo < hi {
        return Some((hi - 1, intervals.len()));
    }
    let mut count = vec![0i64; dates + 1];
    for (s, e) in intervals {
        count[*s] += 1;
        count[*e] -= 1;
    }
    let mut best = (0usize, 0usize);
    let mut running = 0i64;
    for (i, c) in count.iter().take(dates).enumerate() {
        running += c;
        if running as usize >= best.1 {
            best = (i, running as usize);
        }
    }
    Some(best)
}

/// renv.lock from rok.lock (unmanaged packages are left out: renv cannot restore them from a
/// repository either).
pub fn export(lock: &Lockfile) -> String {
    let base = lock.snapshot.repository.trim_end_matches('/');
    let mut repositories: BTreeMap<String, String> = BTreeMap::new();
    let mut packages = Map::new();
    for p in &lock.packages {
        let mut entry = Map::new();
        entry.insert("Package".into(), json!(p.name));
        entry.insert("Version".into(), json!(p.version.to_string()));
        match &p.source {
            Source::Repository {
                repository,
                url,
                snapshot,
                ..
            } => {
                let (name, url) = match (repository.as_str(), snapshot, url) {
                    ("cran", Some(date), _) => (format!("P3M-{date}"), format!("{base}/{date}")),
                    ("cran", None, _) => (
                        format!("P3M-{}", lock.snapshot.date),
                        format!("{base}/{}", lock.snapshot.date),
                    ),
                    (alias, _, Some(url)) => (alias.to_string(), url.clone()),
                    (alias, _, None) => (alias.to_string(), String::new()),
                };
                if !url.is_empty() {
                    repositories.insert(name.clone(), url);
                }
                entry.insert("Source".into(), json!("Repository"));
                entry.insert("Repository".into(), json!(name));
            }
            Source::GitHub {
                owner,
                repo,
                reference,
                commit,
            } => {
                let r = match reference {
                    GitRef::DefaultBranch => "HEAD",
                    GitRef::Branch(b) => b,
                    GitRef::Tag(t) => t,
                    GitRef::Rev(r) => r,
                };
                entry.insert("Source".into(), json!("GitHub"));
                entry.insert("RemoteType".into(), json!("github"));
                entry.insert("RemoteHost".into(), json!("api.github.com"));
                entry.insert("RemoteUsername".into(), json!(owner));
                entry.insert("RemoteRepo".into(), json!(repo));
                entry.insert("RemoteRef".into(), json!(r));
                entry.insert("RemoteSha".into(), json!(commit));
            }
            Source::Unmanaged => continue,
        }
        if !p.dependencies.is_empty() {
            entry.insert("Requirements".into(), json!(p.dependencies));
        }
        packages.insert(p.name.clone(), Value::Object(entry));
    }
    // The project's own date first; renv looks in this order when a package names none.
    let first = format!("P3M-{}", lock.snapshot.date);
    let mut repos: Vec<Value> = Vec::new();
    if let Some(url) = repositories.remove(&first) {
        repos.push(json!({ "Name": first, "URL": url }));
    }
    repos.extend(
        repositories
            .into_iter()
            .map(|(name, url)| json!({ "Name": name, "URL": url })),
    );
    let doc = json!({
        "R": { "Version": lock.r.to_string(), "Repositories": repos },
        "Packages": packages,
    });
    let mut out = serde_json::to_string_pretty(&doc).expect("serializable");
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RENV: &str = r#"{
  "R": {
    "Version": "4.4.2",
    "Repositories": [
      { "Name": "CRAN", "URL": "https://packagemanager.posit.co/cran/2024-06-03" },
      { "Name": "multiverse", "URL": "https://community.r-multiverse.org/" }
    ]
  },
  "Packages": {
    "fixest": { "Package": "fixest", "Version": "0.12.1", "Source": "Repository", "Repository": "CRAN",
      "Depends": ["R (>= 3.5.0)"], "Imports": ["stats", "dreamerr (>= 1.4.0)", "Rcpp"], "LinkingTo": ["Rcpp"] },
    "dreamerr": { "Package": "dreamerr", "Version": "1.4.0", "Source": "Repository", "Repository": "CRAN" },
    "Rcpp": { "Package": "Rcpp", "Version": "1.0.12", "Source": "Repository", "Repository": "CRAN" },
    "coresynth": { "Package": "coresynth", "Version": "0.3.0", "Source": "GitHub", "RemoteType": "github",
      "RemoteUsername": "yo5uke", "RemoteRepo": "coresynth", "RemoteRef": "v0.3.0",
      "RemoteSha": "0123456789abcdef0123456789abcdef01234567", "Requirements": ["fixest"] },
    "polars": { "Package": "polars", "Version": "1.11.0", "Source": "Repository", "Repository": "multiverse",
      "RemoteType": "repository", "RemoteUrl": "https://github.com/pola-rs/r-polars", "RemoteSha": "abc" },
    "limma": { "Package": "limma", "Version": "3.60.0", "Source": "Bioconductor" },
    "renv": { "Package": "renv", "Version": "1.0.7", "Source": "Repository", "Repository": "CRAN" }
  }
}"#;

    #[test]
    fn reads_renv_lockfiles() {
        let l = RenvLock::parse(RENV).unwrap();
        assert_eq!(l.r.as_str(), "4.4.2");
        let get = |n: &str| l.packages.iter().find(|p| p.name == n).unwrap();
        assert_eq!(get("fixest").dependencies, ["dreamerr", "Rcpp"]);
        assert_eq!(get("coresynth").dependencies, ["fixest"]);
        assert!(
            matches!(&get("coresynth").source, RenvSource::GitHub { reference: Some(r), .. } if r == "v0.3.0")
        );
        assert!(matches!(&get("limma").source, RenvSource::Other(s) if s == "Bioconductor"));
        let RenvSource::Repository { repository, remote } = &get("polars").source else {
            panic!()
        };
        assert_eq!(remote.as_ref().unwrap().sha, "abc");
        assert_eq!(
            l.repository_url(repository.as_deref()).unwrap(),
            "https://community.r-multiverse.org"
        );
        // renv itself and packages others need are not roots.
        assert_eq!(l.roots(), ["coresynth", "limma", "polars"]);
    }

    #[test]
    fn classifies_repository_urls() {
        assert!(is_cran("https://cloud.r-project.org"));
        assert!(is_cran("https://packagemanager.posit.co/cran/2024-06-03"));
        assert!(is_cran("https://p3m.dev/cran/__linux__/noble/latest"));
        assert!(is_cran("https://cran.ism.ac.jp"));
        assert!(!is_cran("https://community.r-multiverse.org"));
        assert!(!is_cran("https://yo5uke.r-universe.dev"));
        assert_eq!(
            date_in_url("https://packagemanager.posit.co/cran/__linux__/noble/2024-06-03")
                .as_deref(),
            Some("2024-06-03")
        );
        assert_eq!(
            date_in_url("https://p3m.dev/cran/__linux__/noble/latest"),
            None
        );
    }

    #[test]
    fn estimates_snapshot_dates() {
        let dates: Vec<String> = ["2024-01-01", "2024-02-01", "2024-03-01", "2024-04-01"]
            .map(String::from)
            .to_vec();
        let v = |s: &str| -> Version { s.parse().unwrap() };
        let releases = [
            (v("1.0"), "2024-01-01".to_string()),
            (v("1.1"), "2024-02-01".to_string()),
            (v("1.2"), "2024-04-01".to_string()),
        ];
        assert_eq!(interval(&releases, &v("1.1"), &dates), Some((1, 3)));
        assert_eq!(interval(&releases, &v("1.2"), &dates), Some((3, 4)));
        assert_eq!(interval(&releases, &v("0.9"), &dates), None);
        // All current at 1..3: the latest common date.
        assert_eq!(best_date(&[(1, 3), (0, 4)], 4), Some((2, 2)));
        // No common date: where most are current (the latest such).
        assert_eq!(best_date(&[(0, 1), (2, 4), (3, 4)], 4), Some((3, 2)));
    }

    #[test]
    fn writes_minimal_renv_lockfiles() {
        let lock = Lockfile::parse(
            "version = 1\ngenerated-by = \"rok\"\n[r]\nversion = \"4.6.1\"\n[snapshot]\ndate = \"2026-10-01\"\nrepository = \"https://packagemanager.posit.co/cran\"\n\
             [[package]]\nname = \"fixest\"\nversion = \"0.12.1\"\nsource = { repository = \"cran\", snapshot = \"2024-06-14\" }\ndependencies = [\"Rcpp\"]\n\
             [[package]]\nname = \"Rcpp\"\nversion = \"1.1.2\"\nsource = { repository = \"cran\", snapshot = \"2026-10-01\" }\n\
             [[package]]\nname = \"praise\"\nversion = \"1.0.0\"\nsource = { github = \"gaborcsardi/praise\", commit = \"094469afaf033af00aeb46527970f1db22f4d49d\" }\n\
             [[package]]\nname = \"limma\"\nversion = \"3.60.0\"\nsource = { unmanaged = true }\n",
        )
        .unwrap();
        let out: Value = serde_json::from_str(&export(&lock)).unwrap();
        assert_eq!(out["R"]["Version"], "4.6.1");
        let repos = out["R"]["Repositories"].as_array().unwrap();
        assert_eq!(repos[0]["Name"], "P3M-2026-10-01");
        assert_eq!(
            repos[0]["URL"],
            "https://packagemanager.posit.co/cran/2026-10-01"
        );
        assert_eq!(repos[1]["Name"], "P3M-2024-06-14");
        assert_eq!(out["Packages"]["fixest"]["Repository"], "P3M-2024-06-14");
        assert_eq!(out["Packages"]["fixest"]["Requirements"], json!(["Rcpp"]));
        assert_eq!(out["Packages"]["praise"]["RemoteRef"], "HEAD");
        assert!(out["Packages"].get("limma").is_none());
        assert!(out["Packages"]["fixest"].get("Hash").is_none());
        // What rok writes, rok reads back.
        let back = RenvLock::parse(&export(&lock)).unwrap();
        assert_eq!(back.packages.len(), 3);
    }
}
