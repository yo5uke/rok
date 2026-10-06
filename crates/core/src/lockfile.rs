//! The lockfile, `rok.lock`.
//!
//! The lockfile is TOML, shared by all operating systems. rok writes it itself so that the
//! output is deterministic: packages and names are sorted, and each package is one
//! `[[package]]` table, which keeps Git diffs small.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::constraint::Constraint;
use crate::manifest::is_valid_date;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A CRAN-like repository: `cran` or an alias from `[repositories]`. `snapshot` is the date
    /// the package was taken from, when it is a dated snapshot.
    Repository {
        repository: String,
        snapshot: Option<String>,
    },
}

/// Error returned for an invalid lockfile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockfileError {
    #[error("rok.lock is not valid: {0}")]
    Syntax(String),
    #[error(
        "rok.lock was written by a newer version of rok (lockfile version {0}); update rok to read it"
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    repository: String,
    snapshot: Option<String>,
}

impl Lockfile {
    pub fn parse(text: &str) -> Result<Self, LockfileError> {
        // Check the format version first, so that a newer lockfile gives a clear message
        // instead of an "unknown field" error.
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| LockfileError::Syntax(e.message().to_string()))?;
        if let Some(v) = table.get("version").and_then(|v| v.as_integer())
            && v > i64::from(FORMAT_VERSION)
        {
            return Err(LockfileError::TooNew(u32::try_from(v).unwrap_or(u32::MAX)));
        }
        let raw: RawLockfile =
            toml::from_str(text).map_err(|e| LockfileError::Syntax(e.message().to_string()))?;
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
        if !is_valid_date(&raw.snapshot.date) {
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
            if let Some(date) = &p.source.snapshot
                && !is_valid_date(date)
            {
                return Err(invalid(format!(
                    "package `{}`: invalid snapshot date `{date}`",
                    p.name
                )));
            }
            if let Some(h) = &p.sha256
                && !(h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(invalid(format!(
                    "package `{}`: invalid sha256 `{h}`",
                    p.name
                )));
            }
            packages.push(LockedPackage {
                name: p.name,
                version,
                source: Source::Repository {
                    repository: p.source.repository,
                    snapshot: p.source.snapshot,
                },
                dependencies: p.dependencies,
                sha256: p.sha256,
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
                    snapshot,
                } => {
                    source.insert("repository", repository.as_str().into());
                    if let Some(date) = snapshot {
                        source.insert("snapshot", date.as_str().into());
                    }
                }
            }
            t["source"] = value(source);
            t["dependencies"] = value(sorted_array(&p.dependencies));
            if let Some(h) = &p.sha256 {
                t["sha256"] = value(h.as_str());
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
