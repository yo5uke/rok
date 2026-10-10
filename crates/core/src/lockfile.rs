//! The lockfile, `rok.lock`.
//!
//! The lockfile is TOML, shared by all operating systems. rok writes it itself so that the
//! output is deterministic: packages and names are sorted, and each package is one
//! `[[package]]` table, which keeps Git diffs small.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::constraint::Constraint;
use crate::manifest::{GitRef, parse_github, toml_error};
use crate::version::Version;

/// File name of the lockfile.
pub const FILE_NAME: &str = "rok.lock";

/// The lockfile format version this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// A validated lockfile.
#[derive(Debug, Clone, PartialEq)]
pub struct Lockfile {
    /// For example `rok 0.1.0`.
    pub generated_by: String,
    /// The exact R version, for example `4.4.2`.
    pub r: Version,
    pub snapshot: Snapshot,
    pub manifest: ManifestCopy,
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub date: String,
    pub repository: String,
}

/// A copy of the declarations, used to tell quickly whether the lockfile is out of date.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestCopy {
    pub dependencies: Vec<String>,
    pub constraints: BTreeMap<String, Constraint>,
    /// Declarations other than plain CRAN, in a canonical form such as
    /// `github:yo5uke/coresynth` or `repo:multiverse` (see `ops::source_key`).
    pub sources: BTreeMap<String, String>,
    /// `[unmanaged]` packages, sorted.
    pub unmanaged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: Version,
    pub source: Source,
    /// Names of the packages this one needs (Depends, Imports, LinkingTo), excluding base R.
    pub dependencies: Vec<String>,
    /// SHA-256 of the OS-independent source tarball, when known.
    pub sha256: Option<String>,
    /// Environment variables set when building from source (they change the result).
    pub env: BTreeMap<String, String>,
    /// System requirements from P3M by distribution (`ubuntu-24.04` → `-dev` packages), so
    /// missing libraries can be explained without the network.
    pub sysreqs: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A CRAN-like repository: `cran` (P3M snapshots) or an alias from `[repositories]`, whose
    /// `url` is recorded so the lockfile alone can reproduce the library. `snapshot` is the P3M
    /// date the package was taken from. `remote` is the Git origin that r-universe records in
    /// DESCRIPTION, kept to rebuild a release the repository no longer has.
    Repository {
        repository: String,
        url: Option<String>,
        snapshot: Option<String>,
        remote: Option<Remote>,
    },
    /// A GitHub repository at a commit. `reference` mirrors the manifest (`branch`, `tag`,
    /// `rev`, or the default branch).
    GitHub {
        owner: String,
        repo: String,
        reference: GitRef,
        commit: String,
    },
    /// Listed in `[unmanaged]`: installed by the user (for example from Bioconductor); rok
    /// only records the installed version.
    Unmanaged,
}

/// The Git origin of a package (`RemoteUrl`, `RemoteSha` and `RemoteSubdir` in its
/// DESCRIPTION).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub url: String,
    pub sha: String,
    /// The package's directory in the Git repository, if not the top.
    pub subdir: Option<String>,
    /// The repository no longer had this release, so it was rebuilt from this commit; the
    /// result may differ from the tarball the repository distributed (requirements, chapter 7).
    pub rebuilt: bool,
}

impl Source {
    /// The P3M snapshot date, for packages from CRAN.
    pub fn snapshot(&self) -> Option<&str> {
        match self {
            Source::Repository { snapshot, .. } => snapshot.as_deref(),
            Source::GitHub { .. } | Source::Unmanaged => None,
        }
    }

    /// A short description for messages: `cran 2026-10-01`, `multiverse`, `github yo5uke/x@abc1234`.
    pub fn describe(&self) -> String {
        match self {
            Source::Repository {
                repository,
                snapshot: Some(d),
                ..
            } => format!("{repository} {d}"),
            Source::Repository { repository, .. } => repository.clone(),
            Source::GitHub {
                owner,
                repo,
                commit,
                ..
            } => format!("github {owner}/{repo}@{}", &commit[..commit.len().min(7)]),
            Source::Unmanaged => "unmanaged".to_string(),
        }
    }
}

/// Error returned for an invalid lockfile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockfileError {
    #[error("rok.lock is not valid TOML at {0}.")]
    Syntax(String),
    #[error(
        "rok.lock was written by a newer version of rok (lockfile version {0}).\nUpdate rok with `rok self update` to read it."
    )]
    TooNew(u32),
    #[error("rok.lock: {0}")]
    Invalid(String),
}

// ---- reading ----

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawLockfile {
    version: u32,
    generated_by: String,
    r: RawR,
    snapshot: RawSnapshot,
    #[serde(default)]
    manifest: RawManifest,
    #[serde(default, rename = "package")]
    packages: Vec<RawPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawR {
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSnapshot {
    date: String,
    repository: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    constraints: BTreeMap<String, String>,
    #[serde(default)]
    sources: BTreeMap<String, String>,
    #[serde(default)]
    unmanaged: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPackage {
    name: String,
    version: String,
    source: RawSource,
    #[serde(default)]
    dependencies: Vec<String>,
    sha256: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    sysreqs: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawSource {
    repository: Option<String>,
    url: Option<String>,
    snapshot: Option<String>,
    remote_url: Option<String>,
    remote_sha: Option<String>,
    remote_subdir: Option<String>,
    #[serde(default)]
    rebuilt_from_git: bool,
    github: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
    commit: Option<String>,
    #[serde(default)]
    unmanaged: bool,
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl RawSource {
    fn into_source(self, package: &str) -> Result<Source, LockfileError> {
        let invalid = |m: &str| LockfileError::Invalid(format!("package `{package}`: {m}"));
        if let Some(date) = &self.snapshot
            && !crate::date::is_valid(date)
        {
            return Err(invalid(&format!("invalid snapshot date `{date}`")));
        }
        if self.unmanaged {
            let others = self.repository.is_some()
                || self.github.is_some()
                || self.url.is_some()
                || self.snapshot.is_some()
                || self.commit.is_some();
            if others {
                return Err(invalid("an unmanaged package has no other source fields"));
            }
            return Ok(Source::Unmanaged);
        }
        match (self.repository, self.github) {
            (Some(repository), None) => {
                if self.branch.is_some()
                    || self.tag.is_some()
                    || self.rev.is_some()
                    || self.commit.is_some()
                {
                    return Err(invalid(
                        "`branch`, `tag`, `rev` and `commit` belong to GitHub sources",
                    ));
                }
                let remote = match (self.remote_url, self.remote_sha) {
                    (Some(url), Some(sha)) => Some(Remote {
                        url,
                        sha,
                        subdir: self.remote_subdir,
                        rebuilt: self.rebuilt_from_git,
                    }),
                    (None, None) if self.remote_subdir.is_none() && !self.rebuilt_from_git => None,
                    (None, None) => {
                        return Err(invalid(
                            "`remote-subdir` and `rebuilt-from-git` need `remote-url` and `remote-sha`",
                        ));
                    }
                    _ => return Err(invalid("`remote-url` and `remote-sha` go together")),
                };
                Ok(Source::Repository {
                    repository,
                    url: self.url,
                    snapshot: self.snapshot,
                    remote,
                })
            }
            (None, Some(github)) => {
                let (owner, repo) = parse_github(&github)
                    .ok_or_else(|| invalid(&format!("invalid GitHub repository `{github}`")))?;
                let commit = self
                    .commit
                    .ok_or_else(|| invalid("a GitHub source needs `commit`"))?;
                if !is_hex(&commit, 40) {
                    return Err(invalid(&format!("invalid commit `{commit}`")));
                }
                let reference = match (self.branch, self.tag, self.rev) {
                    (None, None, None) => GitRef::DefaultBranch,
                    (Some(b), None, None) => GitRef::Branch(b),
                    (None, Some(t), None) => GitRef::Tag(t),
                    (None, None, Some(r)) => GitRef::Rev(r),
                    _ => return Err(invalid("use only one of `branch`, `tag` and `rev`")),
                };
                if self.url.is_some()
                    || self.snapshot.is_some()
                    || self.remote_url.is_some()
                    || self.remote_sha.is_some()
                    || self.remote_subdir.is_some()
                    || self.rebuilt_from_git
                {
                    return Err(invalid(
                        "`url`, `snapshot` and `remote-*` belong to repository sources",
                    ));
                }
                Ok(Source::GitHub {
                    owner,
                    repo,
                    reference,
                    commit,
                })
            }
            _ => Err(invalid("a source needs either `repository` or `github`")),
        }
    }
}

impl Lockfile {
    pub fn parse(text: &str) -> Result<Self, LockfileError> {
        // Check the format version first, so that a newer lockfile gives a clear message
        // instead of an "unknown field" error.
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| LockfileError::Syntax(toml_error(text, &e)))?;
        if let Some(v) = table.get("version").and_then(|v| v.as_integer())
            && v > i64::from(FORMAT_VERSION)
        {
            return Err(LockfileError::TooNew(u32::try_from(v).unwrap_or(u32::MAX)));
        }
        let raw: RawLockfile =
            toml::from_str(text).map_err(|e| LockfileError::Syntax(toml_error(text, &e)))?;
        if raw.version != FORMAT_VERSION {
            return Err(LockfileError::Invalid(format!(
                "unsupported lockfile version {}",
                raw.version
            )));
        }

        let invalid = |m: String| LockfileError::Invalid(m);
        let r =
            raw.r.version.parse().map_err(|_| {
                invalid(format!("`r.version`: invalid version `{}`", raw.r.version))
            })?;
        if !crate::date::is_valid(&raw.snapshot.date) {
            return Err(invalid(format!(
                "`snapshot.date`: invalid date `{}`",
                raw.snapshot.date
            )));
        }
        let mut constraints = BTreeMap::new();
        for (name, c) in raw.manifest.constraints {
            let c = c.parse().map_err(|_| {
                invalid(format!(
                    "`manifest.constraints.{name}`: invalid constraint `{c}`"
                ))
            })?;
            constraints.insert(name, c);
        }
        let mut packages = Vec::with_capacity(raw.packages.len());
        for p in raw.packages {
            let version = p.version.parse().map_err(|_| {
                invalid(format!(
                    "package `{}`: invalid version `{}`",
                    p.name, p.version
                ))
            })?;
            if let Some(h) = &p.sha256
                && !is_hex(h, 64)
            {
                return Err(invalid(format!(
                    "package `{}`: invalid sha256 `{h}`",
                    p.name
                )));
            }
            let source = p.source.into_source(&p.name)?;
            packages.push(LockedPackage {
                name: p.name,
                version,
                source,
                dependencies: p.dependencies,
                sha256: p.sha256,
                env: p.env,
                sysreqs: p.sysreqs,
            });
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(dup) = packages.iter().find(|p| !seen.insert(p.name.as_str())) {
            return Err(invalid(format!(
                "package `{}` appears more than once",
                dup.name
            )));
        }

        Ok(Lockfile {
            generated_by: raw.generated_by,
            r,
            snapshot: Snapshot {
                date: raw.snapshot.date,
                repository: raw.snapshot.repository,
            },
            manifest: ManifestCopy {
                dependencies: raw.manifest.dependencies,
                constraints,
                sources: raw.manifest.sources,
                unmanaged: raw.manifest.unmanaged,
            },
            packages,
        })
    }

    pub fn package(&self, name: &str) -> Option<&LockedPackage> {
        self.packages.iter().find(|p| p.name == name)
    }

    /// Serializes the lockfile. The output does not depend on the order of the input.
    pub fn to_toml_string(&self) -> String {
        use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, value};

        let sorted_array = |names: &[String]| -> Array {
            let mut names: Vec<&String> = names.iter().collect();
            names.sort_by(|a, b| name_order(a, b));
            names.dedup();
            names.into_iter().map(String::as_str).collect()
        };

        let mut doc = DocumentMut::new();
        doc["version"] = value(i64::from(FORMAT_VERSION));
        doc["generated-by"] = value(self.generated_by.as_str());

        let mut r = Table::new();
        r["version"] = value(self.r.as_str());
        doc["r"] = Item::Table(r);

        let mut snapshot = Table::new();
        snapshot["date"] = value(self.snapshot.date.as_str());
        snapshot["repository"] = value(self.snapshot.repository.as_str());
        doc["snapshot"] = Item::Table(snapshot);

        let mut manifest = Table::new();
        manifest["dependencies"] = value(sorted_array(&self.manifest.dependencies));
        let mut constraints = InlineTable::new();
        let mut names: Vec<&String> = self.manifest.constraints.keys().collect();
        names.sort_by(|a, b| name_order(a, b));
        for name in names {
            constraints.insert(name, self.manifest.constraints[name].to_string().into());
        }
        manifest["constraints"] = value(constraints);
        if !self.manifest.sources.is_empty() {
            let mut sources = InlineTable::new();
            let mut names: Vec<&String> = self.manifest.sources.keys().collect();
            names.sort_by(|a, b| name_order(a, b));
            for name in names {
                sources.insert(name, self.manifest.sources[name].as_str().into());
            }
            manifest["sources"] = value(sources);
        }
        if !self.manifest.unmanaged.is_empty() {
            manifest["unmanaged"] = value(sorted_array(&self.manifest.unmanaged));
        }
        doc["manifest"] = Item::Table(manifest);

        let mut packages: Vec<&LockedPackage> = self.packages.iter().collect();
        packages.sort_by(|a, b| name_order(&a.name, &b.name));
        let mut array = ArrayOfTables::new();
        for p in packages {
            let mut t = Table::new();
            t["name"] = value(p.name.as_str());
            t["version"] = value(p.version.as_str());
            let mut source = InlineTable::new();
            match &p.source {
                Source::Repository {
                    repository,
                    url,
                    snapshot,
                    remote,
                } => {
                    source.insert("repository", repository.as_str().into());
                    if let Some(url) = url {
                        source.insert("url", url.as_str().into());
                    }
                    if let Some(date) = snapshot {
                        source.insert("snapshot", date.as_str().into());
                    }
                    if let Some(r) = remote {
                        source.insert("remote-url", r.url.as_str().into());
                        source.insert("remote-sha", r.sha.as_str().into());
                        if let Some(d) = &r.subdir {
                            source.insert("remote-subdir", d.as_str().into());
                        }
                        if r.rebuilt {
                            source.insert("rebuilt-from-git", true.into());
                        }
                    }
                }
                Source::GitHub {
                    owner,
                    repo,
                    reference,
                    commit,
                } => {
                    source.insert("github", format!("{owner}/{repo}").into());
                    match reference {
                        GitRef::DefaultBranch => {}
                        GitRef::Branch(b) => {
                            source.insert("branch", b.as_str().into());
                        }
                        GitRef::Tag(t) => {
                            source.insert("tag", t.as_str().into());
                        }
                        GitRef::Rev(r) => {
                            source.insert("rev", r.as_str().into());
                        }
                    }
                    source.insert("commit", commit.as_str().into());
                }
                Source::Unmanaged => {
                    source.insert("unmanaged", true.into());
                }
            }
            t["source"] = value(source);
            t["dependencies"] = value(sorted_array(&p.dependencies));
            if let Some(h) = &p.sha256 {
                t["sha256"] = value(h.as_str());
            }
            if !p.env.is_empty() {
                let mut env = InlineTable::new();
                for (k, v) in &p.env {
                    env.insert(k, v.as_str().into());
                }
                t["env"] = value(env);
            }
            if !p.sysreqs.is_empty() {
                let mut reqs = InlineTable::new();
                for (distro, packages) in &p.sysreqs {
                    reqs.insert(distro, sorted_array(packages).into());
                }
                t["sysreqs"] = value(reqs);
            }
            array.push(t);
        }
        doc["package"] = Item::ArrayOfTables(array);
        doc.to_string()
    }
}

/// The order used for package names in rok files: case-insensitive, then case-sensitive.
pub fn name_order(a: &str, b: &str) -> Ordering {
    a.to_ascii_lowercase()
        .cmp(&b.to_ascii_lowercase())
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"version = 1
generated-by = "rok 0.1.0"

[r]
version = "4.4.2"

[snapshot]
date = "2026-10-04"
repository = "https://packagemanager.posit.co/cran"

[manifest]
dependencies = ["coresynth", "fixest", "sf"]
constraints = { fixest = "< 0.13" }

[[package]]
name = "data.table"
version = "1.18.6.1"
source = { repository = "cran", snapshot = "2026-10-04" }
dependencies = []

[[package]]
name = "fixest"
version = "0.12.1"
source = { repository = "cran", snapshot = "2026-07-15" }
dependencies = ["data.table", "dreamerr", "Rcpp"]
sha256 = "f2846f45fbbdfe886f05fa7489314a56ae0f3352ae5bc1070b0cdd4f44ffa1f5"
"#;

    #[test]
    fn round_trips_the_documented_format() {
        let lock = Lockfile::parse(EXAMPLE).unwrap();
        assert_eq!(lock.r.as_str(), "4.4.2");
        assert_eq!(lock.manifest.constraints["fixest"].to_string(), "< 0.13");
        assert_eq!(lock.package("fixest").unwrap().version.as_str(), "0.12.1");
        assert_eq!(lock.package("data.table").unwrap().sha256, None);
        assert_eq!(lock.to_toml_string(), EXAMPLE);
    }

    const SOURCES: &str = r#"version = 1
generated-by = "rok 0.1.0"

[r]
version = "4.6.1"

[snapshot]
date = "2026-10-01"
repository = "https://packagemanager.posit.co/cran"

[manifest]
dependencies = ["coresynth", "polars"]
constraints = {}
sources = { coresynth = "github:yo5uke/coresynth", polars = "repo:multiverse" }

[[package]]
name = "coresynth"
version = "0.3.0"
source = { github = "yo5uke/coresynth", tag = "v0.3.0", commit = "0123456789abcdef0123456789abcdef01234567" }
dependencies = []

[[package]]
name = "limma"
version = "3.60.0"
source = { unmanaged = true }
dependencies = []

[[package]]
name = "polars"
version = "1.16.0"
source = { repository = "multiverse", url = "https://community.r-multiverse.org", remote-url = "https://github.com/pola-rs/r-polars", remote-sha = "abc", remote-subdir = "pkg", rebuilt-from-git = true }
dependencies = []
sha256 = "f263f133f75fc9501c356e24cf2a0fac1dfa0d6d01a121ef3eaa79d9886bbd22"
env = { NOT_CRAN = "true" }
sysreqs = { "ubuntu-24.04" = ["cargo", "rustc"] }
"#;

    #[test]
    fn round_trips_github_and_repository_sources() {
        let lock = Lockfile::parse(SOURCES).unwrap();
        assert!(
            matches!(&lock.package("coresynth").unwrap().source, Source::GitHub { reference: GitRef::Tag(t), .. } if t == "v0.3.0")
        );
        assert_eq!(lock.package("polars").unwrap().env["NOT_CRAN"], "true");
        assert_eq!(
            lock.package("coresynth").unwrap().source.describe(),
            "github yo5uke/coresynth@0123456"
        );
        assert_eq!(lock.to_toml_string(), SOURCES);
        let bad = SOURCES.replace(
            "commit = \"0123456789abcdef0123456789abcdef01234567\"",
            "commit = \"main\"",
        );
        assert!(
            Lockfile::parse(&bad)
                .unwrap_err()
                .to_string()
                .contains("invalid commit")
        );
        let both = SOURCES.replace(
            "{ github = \"yo5uke/coresynth\",",
            "{ repository = \"cran\", github = \"yo5uke/coresynth\",",
        );
        assert!(Lockfile::parse(&both).is_err());
    }

    #[test]
    fn output_is_sorted_and_independent_of_input_order() {
        let mut lock = Lockfile::parse(EXAMPLE).unwrap();
        lock.packages.reverse();
        lock.packages[0].dependencies = vec![
            "Rcpp".into(),
            "dreamerr".into(),
            "data.table".into(),
            "Rcpp".into(),
        ];
        lock.manifest.dependencies = vec!["sf".into(), "fixest".into(), "coresynth".into()];
        assert_eq!(lock.to_toml_string(), EXAMPLE);
    }

    #[test]
    fn sorts_names_case_insensitively() {
        let mut names = vec!["Rcpp", "data.table", "R6", "dreamerr", "rlang", "RCurl"];
        names.sort_by(|a, b| name_order(a, b));
        assert_eq!(
            names,
            ["data.table", "dreamerr", "R6", "Rcpp", "RCurl", "rlang"]
        );
    }

    #[test]
    fn rejects_invalid_lockfiles() {
        let err = |text: &str| Lockfile::parse(text).unwrap_err();
        assert_eq!(
            err(&EXAMPLE.replacen("version = 1", "version = 2", 1)),
            LockfileError::TooNew(2)
        );
        assert!(matches!(
            err(&EXAMPLE.replace("generated-by", "generator")),
            LockfileError::Syntax(_)
        ));
        assert!(
            err(&EXAMPLE.replace("0.12.1", "zero"))
                .to_string()
                .contains("invalid version")
        );
        assert!(
            err(&EXAMPLE.replace("\"2026-07-15\"", "\"2026-07-32\""))
                .to_string()
                .contains("invalid snapshot date")
        );
        assert!(
            err(&EXAMPLE.replace("f2846f45", "nothex!!"))
                .to_string()
                .contains("invalid sha256")
        );
        assert!(
            err(&EXAMPLE.replace("name = \"data.table\"", "name = \"fixest\""))
                .to_string()
                .contains("more than once")
        );
    }
}
