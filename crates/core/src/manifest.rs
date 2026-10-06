//! The project manifest, `rok.toml`.
//!
//! Users may edit the manifest by hand, so reading validates every field and reports the
//! offending key, and writing goes through [`ManifestDocument`], which changes only the keys
//! rok manages and keeps comments and formatting.

use std::collections::BTreeMap;
use std::fmt;

use crate::constraint::Constraint;
use crate::dcf::is_package_name;
use crate::version::Version;

/// File name of the manifest.
pub const FILE_NAME: &str = "rok.toml";

/// A validated manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub project: Project,
    /// Declared packages, in file order.
    pub dependencies: Vec<(String, DependencySpec)>,
    /// CRAN-like repositories other than CRAN: alias → URL.
    pub repositories: BTreeMap<String, String>,
    pub sync: SyncSettings,
    pub unmanaged: Vec<String>,
    pub scan_rules: Vec<ScanRule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    /// R version: a minor version (`4.4`) or, if pinned, a patch version (`4.4.2`).
    pub r: Version,
    /// Snapshot date, `YYYY-MM-DD`.
    pub snapshot: String,
}

/// How one declared package is obtained.
#[derive(Debug, Clone, PartialEq)]
pub struct DependencySpec {
    pub source: DependencySource,
    /// Environment variables set when building from source.
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DependencySource {
    /// CRAN through the P3M snapshot, with an optional version constraint.
    Cran { constraint: Constraint },
    /// A CRAN-like repository declared in `[repositories]`, with an optional version constraint.
    Repository {
        alias: String,
        constraint: Constraint,
    },
    /// A GitHub repository.
    GitHub {
        owner: String,
        repo: String,
        reference: GitRef,
        track: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRef {
    DefaultBranch,
    Branch(String),
    Tag(String),
    Rev(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnStartup {
    #[default]
    Auto,
    Ask,
    Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NonInteractive {
    #[default]
    Warn,
    Error,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncSettings {
    pub on_startup: OnStartup,
    pub noninteractive: NonInteractive,
}

/// A project-specific scanner rule (`[[scan.rule]]`). The condition is checked by the scanner.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRule {
    pub when: toml::Table,
    pub suggest: Vec<String>,
}

/// Error returned for an invalid manifest.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("rok.toml is not valid TOML: {0}")]
    Syntax(String),
    #[error("rok.toml: `{key}`: {message}")]
    Invalid { key: String, message: String },
}

fn invalid(key: impl Into<String>, message: impl Into<String>) -> ManifestError {
    ManifestError::Invalid {
        key: key.into(),
        message: message.into(),
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let doc: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| ManifestError::Syntax(e.message().to_string()))?;
        check_keys(
            &doc,
            "",
            &[
                "project",
                "dependencies",
                "repositories",
                "sync",
                "unmanaged",
                "scan",
            ],
        )?;

        let project = table(&doc, "project")?.ok_or_else(|| invalid("project", "missing table"))?;
        check_keys(project, "project.", &["name", "r", "snapshot"])?;
        let name = required_str(project, "project.name")?;
        if name.trim().is_empty() {
            return Err(invalid("project.name", "must not be empty"));
        }
        let r_text = required_str(project, "project.r")?;
        let r: Version = r_text
            .parse()
            .ok()
            .filter(|v: &Version| matches!(v.parts().len(), 2 | 3) && !r_text.contains('-'))
            .ok_or_else(|| {
                invalid(
                    "project.r",
                    format!("expected an R version such as `4.4` or `4.4.2`, found `{r_text}`"),
                )
            })?;
        let snapshot = required_str(project, "project.snapshot")?;
        if !is_valid_date(snapshot) {
            return Err(invalid(
                "project.snapshot",
                format!("expected a date such as `2026-10-04`, found `{snapshot}`"),
            ));
        }

        let mut repositories = BTreeMap::new();
        if let Some(repos) = table(&doc, "repositories")? {
            for (alias, url) in repos {
                let key = format!("repositories.{alias}");
                let url = url
                    .as_str()
                    .ok_or_else(|| invalid(&key, "expected a URL string"))?;
                if alias == "cran" {
                    return Err(invalid(
                        &key,
                        "`cran` is reserved for CRAN; choose another name",
                    ));
                }
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(invalid(
                        &key,
                        format!("expected an http(s) URL, found `{url}`"),
                    ));
                }
                repositories.insert(alias.clone(), url.trim_end_matches('/').to_string());
            }
        }

        let mut dependencies = Vec::new();
        if let Some(deps) = table(&doc, "dependencies")? {
            for (name, value) in deps {
                let key = format!("dependencies.{name}");
                if !is_package_name(name) {
                    return Err(invalid(&key, "not a valid R package name"));
                }
                dependencies.push((name.clone(), parse_dependency(&key, value, &repositories)?));
            }
        }

        let mut sync = SyncSettings::default();
        if let Some(t) = table(&doc, "sync")? {
            check_keys(t, "sync.", &["on_startup", "noninteractive"])?;
            if let Some(v) = optional_str(t, "sync.on_startup")? {
                sync.on_startup = match v {
                    "auto" => OnStartup::Auto,
                    "ask" => OnStartup::Ask,
                    "notify" => OnStartup::Notify,
                    _ => {
                        return Err(invalid(
                            "sync.on_startup",
                            format!("expected `auto`, `ask` or `notify`, found `{v}`"),
                        ));
                    }
                };
            }
            if let Some(v) = optional_str(t, "sync.noninteractive")? {
                sync.noninteractive = match v {
                    "warn" => NonInteractive::Warn,
                    "error" => NonInteractive::Error,
                    "auto" => NonInteractive::Auto,
                    _ => {
                        return Err(invalid(
                            "sync.noninteractive",
                            format!("expected `warn`, `error` or `auto`, found `{v}`"),
                        ));
                    }
                };
            }
        }

        let mut unmanaged = Vec::new();
        if let Some(t) = table(&doc, "unmanaged")? {
            check_keys(t, "unmanaged.", &["packages"])?;
            if let Some(v) = t.get("packages") {
                unmanaged = package_list("unmanaged.packages", v)?;
            }
        }

        let mut scan_rules = Vec::new();
        if let Some(t) = table(&doc, "scan")? {
            check_keys(t, "scan.", &["rule"])?;
            if let Some(rules) = t.get("rule") {
                let rules = rules.as_array().ok_or_else(|| {
                    invalid("scan.rule", "expected an array of tables (`[[scan.rule]]`)")
                })?;
                for (i, rule) in rules.iter().enumerate() {
                    let key = format!("scan.rule[{i}]");
                    let rule = rule
                        .as_table()
                        .ok_or_else(|| invalid(&key, "expected a table"))?;
                    check_keys(rule, &format!("{key}."), &["when", "suggest"])?;
                    let when = rule
                        .get("when")
                        .and_then(|w| w.as_table())
                        .ok_or_else(|| invalid(format!("{key}.when"), "expected a table"))?
                        .clone();
                    let suggest = package_list(
                        &format!("{key}.suggest"),
                        rule.get("suggest")
                            .ok_or_else(|| invalid(format!("{key}.suggest"), "missing"))?,
                    )?;
                    scan_rules.push(ScanRule { when, suggest });
                }
            }
        }

        Ok(Manifest {
            project: Project {
                name: name.to_string(),
                r,
                snapshot: snapshot.to_string(),
            },
            dependencies,
            repositories,
            sync,
            unmanaged,
            scan_rules,
        })
    }

    pub fn dependency(&self, name: &str) -> Option<&DependencySpec> {
        self.dependencies
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
    }
}

fn parse_dependency(
    key: &str,
    value: &toml::Value,
    repositories: &BTreeMap<String, String>,
) -> Result<DependencySpec, ManifestError> {
    if let Some(s) = value.as_str() {
        let constraint = s
            .parse()
            .map_err(|e: crate::constraint::ParseConstraintError| invalid(key, e.to_string()))?;
        return Ok(DependencySpec {
            source: DependencySource::Cran { constraint },
            env: BTreeMap::new(),
        });
    }
    let t = value
        .as_table()
        .ok_or_else(|| invalid(key, "expected a version constraint string or a table"))?;
    check_keys(
        t,
        &format!("{key}."),
        &[
            "version", "repo", "github", "branch", "tag", "rev", "track", "env",
        ],
    )?;
    let sub = |k: &str| format!("{key}.{k}");

    let mut env = BTreeMap::new();
    if let Some(e) = t.get("env") {
        let e = e
            .as_table()
            .ok_or_else(|| invalid(sub("env"), "expected a table of strings"))?;
        for (name, v) in e {
            let v = v
                .as_str()
                .ok_or_else(|| invalid(format!("{key}.env.{name}"), "expected a string"))?;
            env.insert(name.clone(), v.to_string());
        }
    }

    let constraint = match optional_str(t, &sub("version"))? {
        None => None,
        Some(v) => Some(
            v.parse::<Constraint>()
                .map_err(|e| invalid(sub("version"), e.to_string()))?,
        ),
    };
    let repo = optional_str(t, &sub("repo"))?;
    let github = t
        .get("github")
        .map(|v| {
            v.as_str()
                .ok_or_else(|| invalid(sub("github"), "expected a string"))
        })
        .transpose()?;
    let git_key = |k: &str| {
        t.get(k)
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| invalid(sub(k), "expected a string"))
            })
            .transpose()
    };
    let (branch, tag, rev) = (git_key("branch")?, git_key("tag")?, git_key("rev")?);
    let track = match t.get("track") {
        None => None,
        Some(v) => Some(
            v.as_bool()
                .ok_or_else(|| invalid(sub("track"), "expected `true` or `false`"))?,
        ),
    };

    if github.is_none() && (branch.is_some() || tag.is_some() || rev.is_some() || track.is_some()) {
        return Err(invalid(
            key,
            "`branch`, `tag`, `rev` and `track` can only be used with `github`",
        ));
    }
    let source = match (repo, github) {
        (Some(_), Some(_)) => return Err(invalid(key, "use either `repo` or `github`, not both")),
        (None, None) => {
            if constraint.is_none() && env.is_empty() {
                return Err(invalid(key, "expected `version`, `repo` or `github`"));
            }
            DependencySource::Cran {
                constraint: constraint.unwrap_or_default(),
            }
        }
        (Some(alias), None) => {
            if !repositories.contains_key(alias) {
                return Err(invalid(
                    sub("repo"),
                    format!("`{alias}` is not declared in [repositories]"),
                ));
            }
            DependencySource::Repository {
                alias: alias.to_string(),
                constraint: constraint.unwrap_or_default(),
            }
        }
        (None, Some(gh)) => {
            if constraint.is_some() {
                return Err(invalid(
                    sub("version"),
                    "cannot be used with `github`; pin a version with `tag` or `rev`",
                ));
            }
            let (owner, repo) = parse_github(gh).ok_or_else(|| {
                invalid(
                    sub("github"),
                    format!("expected `owner/repo`, found `{gh}`"),
                )
            })?;
            let reference = match (branch, tag, rev) {
                (None, None, None) => GitRef::DefaultBranch,
                (Some(b), None, None) => GitRef::Branch(b),
                (None, Some(t), None) => GitRef::Tag(t),
                (None, None, Some(r)) => GitRef::Rev(r),
                _ => return Err(invalid(key, "use only one of `branch`, `tag` and `rev`")),
            };
            let track = track.unwrap_or(false);
            if track && matches!(reference, GitRef::Tag(_) | GitRef::Rev(_)) {
                return Err(invalid(
                    sub("track"),
                    "`track = true` only works with a branch, not with `tag` or `rev`",
                ));
            }
            DependencySource::GitHub {
                owner,
                repo,
                reference,
                track,
            }
        }
    };
    Ok(DependencySpec { source, env })
}

/// Splits `owner/repo`. Both parts must be non-empty and use GitHub's allowed characters.
pub fn parse_github(s: &str) -> Option<(String, String)> {
    let (owner, repo) = s.split_once('/')?;
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (ok(owner) && ok(repo)).then(|| (owner.to_string(), repo.to_string()))
}

fn table<'a>(t: &'a toml::Table, key: &str) -> Result<Option<&'a toml::Table>, ManifestError> {
    match t.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_table()
            .map(Some)
            .ok_or_else(|| invalid(key, "expected a table")),
    }
}

fn required_str<'a>(t: &'a toml::Table, key: &str) -> Result<&'a str, ManifestError> {
    optional_str(t, key)?.ok_or_else(|| invalid(key, "missing"))
}

fn optional_str<'a>(t: &'a toml::Table, key: &str) -> Result<Option<&'a str>, ManifestError> {
    let short = key.rsplit('.').next().unwrap_or(key);
    match t.get(short) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| invalid(key, "expected a string")),
    }
}

fn check_keys(t: &toml::Table, prefix: &str, allowed: &[&str]) -> Result<(), ManifestError> {
    match t.keys().find(|k| !allowed.contains(&k.as_str())) {
        None => Ok(()),
        Some(k) => Err(invalid(
            format!("{prefix}{k}"),
            format!("unknown key (expected one of: {})", allowed.join(", ")),
        )),
    }
}

fn package_list(key: &str, v: &toml::Value) -> Result<Vec<String>, ManifestError> {
    let arr = v
        .as_array()
        .ok_or_else(|| invalid(key, "expected an array of package names"))?;
    arr.iter()
        .map(|p| {
            p.as_str()
                .filter(|s| is_package_name(s))
                .map(str::to_string)
                .ok_or_else(|| invalid(key, format!("not a valid R package name: {p}")))
        })
        .collect()
}

/// Whether `s` is a calendar date in `YYYY-MM-DD` form.
pub fn is_valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    // Bytes 4 and 7 are ASCII `-`, so the slices below fall on character boundaries.
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = &s[r];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

impl DependencySpec {
    /// A CRAN dependency with the given constraint.
    pub fn cran(constraint: Constraint) -> Self {
        DependencySpec {
            source: DependencySource::Cran { constraint },
            env: BTreeMap::new(),
        }
    }

    /// The TOML value written to `[dependencies]`: a constraint string for a plain CRAN
    /// dependency, otherwise an inline table. `version` is omitted when it allows any version.
    fn to_toml_value(&self) -> toml_edit::Value {
        let mut t = toml_edit::InlineTable::new();
        match &self.source {
            DependencySource::Cran { constraint } => {
                if self.env.is_empty() {
                    return constraint.to_string().into();
                }
                if !constraint.is_any() {
                    t.insert("version", constraint.to_string().into());
                }
            }
            DependencySource::Repository { alias, constraint } => {
                t.insert("repo", alias.as_str().into());
                if !constraint.is_any() {
                    t.insert("version", constraint.to_string().into());
                }
            }
            DependencySource::GitHub {
                owner,
                repo,
                reference,
                track,
            } => {
                t.insert("github", format!("{owner}/{repo}").into());
                match reference {
                    GitRef::DefaultBranch => {}
                    GitRef::Branch(b) => {
                        t.insert("branch", b.as_str().into());
                    }
                    GitRef::Tag(v) => {
                        t.insert("tag", v.as_str().into());
                    }
                    GitRef::Rev(r) => {
                        t.insert("rev", r.as_str().into());
                    }
                }
                if *track {
                    t.insert("track", true.into());
                }
            }
        }
        if !self.env.is_empty() {
            let mut env = toml_edit::InlineTable::new();
            for (k, v) in &self.env {
                env.insert(k, v.as_str().into());
            }
            t.insert("env", env.into());
        }
        t.into()
    }
}

/// A manifest kept as a TOML document, so edits preserve the user's comments and layout.
#[derive(Debug, Clone)]
pub struct ManifestDocument {
    doc: toml_edit::DocumentMut,
}

impl ManifestDocument {
    /// Parses and validates `text`, returning both the editable document and its contents.
    pub fn parse(text: &str) -> Result<(Self, Manifest), ManifestError> {
        let manifest = Manifest::parse(text)?;
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| ManifestError::Syntax(e.to_string()))?;
        Ok((ManifestDocument { doc }, manifest))
    }

    /// Adds or replaces a dependency. A new one is appended at the end of `[dependencies]`;
    /// an existing one keeps its position.
    pub fn set_dependency(&mut self, name: &str, spec: &DependencySpec) {
        let deps = self.doc.entry("dependencies").or_insert_with(|| {
            let mut t = toml_edit::Table::new();
            t.set_implicit(false);
            toml_edit::Item::Table(t)
        });
        let table = deps
            .as_table_like_mut()
            .expect("[dependencies] was validated as a table");
        match table.get_mut(name) {
            Some(item) => {
                // Keep the surrounding whitespace and trailing comment of the old value.
                let decor = item.as_value().map(|v| v.decor().clone());
                let mut value = spec.to_toml_value();
                if let Some(decor) = decor {
                    *value.decor_mut() = decor;
                }
                *item = toml_edit::Item::Value(value);
            }
            None => {
                table.insert(name, toml_edit::Item::Value(spec.to_toml_value()));
            }
        }
    }

    /// Removes a dependency. Returns whether it was declared.
    pub fn remove_dependency(&mut self, name: &str) -> bool {
        self.doc
            .get_mut("dependencies")
            .and_then(|d| d.as_table_like_mut())
            .and_then(|t| t.remove(name))
            .is_some()
    }

    pub fn set_snapshot(&mut self, date: &str) {
        self.set_project_value("snapshot", date);
    }

    pub fn set_r(&mut self, r: &Version) {
        self.set_project_value("r", r.as_str());
    }

    fn set_project_value(&mut self, key: &str, value: &str) {
        let project = self.doc["project"]
            .as_table_like_mut()
            .expect("[project] was validated as a table");
        match project.get_mut(key).and_then(|i| i.as_value_mut()) {
            Some(v) => {
                let decor = v.decor().clone();
                *v = value.into();
                *v.decor_mut() = decor;
            }
            None => {
                project.insert(key, toml_edit::value(value));
            }
        }
    }
}

impl fmt::Display for ManifestDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"# My analysis
[project]
name = "my-project"
r = "4.4"
snapshot = "2026-10-04"

[repositories]
multiverse = "https://community.r-multiverse.org/"

[dependencies]
fixest = "< 0.13"   # keep the old API
sf = "*"
coresynth = { github = "yo5uke/coresynth" }
fixes = { github = "yo5uke/fixes", track = true }
pkgB = { github = "user/pkgB", tag = "v0.3.0" }
polars = { repo = "multiverse", env = { NOT_CRAN = "true" } }
tidypolars = { repo = "multiverse", version = ">= 0.10" }
arrow = { version = "< 20.0", env = { LIBARROW_BINARY = "true" } }

[sync]
on_startup = "ask"
noninteractive = "error"

[unmanaged]
packages = ["DESeq2", "limma"]

[[scan.rule]]
when = { call = "mytools::render_report" }
suggest = ["officer", "flextable"]
"#;

    fn parse_err(text: &str) -> String {
        Manifest::parse(text).unwrap_err().to_string()
    }

    fn with_deps(deps: &str) -> String {
        format!(
            "[project]\nname = \"p\"\nr = \"4.4\"\nsnapshot = \"2026-10-04\"\n[repositories]\nmv = \"https://m.org\"\n[dependencies]\n{deps}\n"
        )
    }

    #[test]
    fn parses_the_documented_example() {
        let m = Manifest::parse(EXAMPLE).unwrap();
        assert_eq!(m.project.name, "my-project");
        assert_eq!(m.project.r.as_str(), "4.4");
        let names: Vec<_> = m.dependencies.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "fixest",
                "sf",
                "coresynth",
                "fixes",
                "pkgB",
                "polars",
                "tidypolars",
                "arrow"
            ]
        );
        assert!(
            matches!(&m.dependency("fixest").unwrap().source, DependencySource::Cran { constraint } if constraint.to_string() == "< 0.13")
        );
        assert!(matches!(
            &m.dependency("fixes").unwrap().source,
            DependencySource::GitHub {
                track: true,
                reference: GitRef::DefaultBranch,
                ..
            }
        ));
        assert!(
            matches!(&m.dependency("pkgB").unwrap().source, DependencySource::GitHub { reference: GitRef::Tag(t), .. } if t == "v0.3.0")
        );
        let polars = m.dependency("polars").unwrap();
        assert_eq!(
            polars.source,
            DependencySource::Repository {
                alias: "multiverse".into(),
                constraint: Constraint::any()
            }
        );
        assert!(matches!(
            &m.dependency("tidypolars").unwrap().source,
            DependencySource::Repository { constraint, .. } if constraint.to_string() == ">= 0.10"
        ));
        let arrow = m.dependency("arrow").unwrap();
        assert!(matches!(
            &arrow.source,
            DependencySource::Cran { constraint } if constraint.to_string() == "< 20.0"
        ));
        assert_eq!(arrow.env["LIBARROW_BINARY"], "true");
        assert_eq!(polars.env.get("NOT_CRAN").map(String::as_str), Some("true"));
        assert_eq!(
            m.repositories["multiverse"],
            "https://community.r-multiverse.org"
        );
        assert_eq!(m.sync.on_startup, OnStartup::Ask);
        assert_eq!(m.sync.noninteractive, NonInteractive::Error);
        assert_eq!(m.unmanaged, ["DESeq2", "limma"]);
        assert_eq!(m.scan_rules[0].suggest, ["officer", "flextable"]);
    }

    #[test]
    fn applies_defaults() {
        let m = Manifest::parse(&with_deps("")).unwrap();
        assert!(m.dependencies.is_empty());
        assert_eq!(m.sync, SyncSettings::default());
        assert_eq!(m.sync.on_startup, OnStartup::Auto);
        assert_eq!(m.sync.noninteractive, NonInteractive::Warn);
    }

    #[test]
    fn reports_invalid_fields_by_key() {
        assert!(parse_err("[project\n").contains("not valid TOML"));
        assert!(parse_err("[dependencies]\n").contains("`project`: missing table"));
        let base = "[project]\nname = \"p\"\nr = \"4.4\"\nsnapshot = \"2026-10-04\"\n";
        assert!(parse_err(&base.replace("4.4\"", "latest\"")).contains("`project.r`"));
        assert!(parse_err(&base.replace("4.4\"", "4\"")).contains("`project.r`"));
        assert!(
            parse_err(&base.replace("2026-10-04", "2026-02-30")).contains("`project.snapshot`")
        );
        assert!(parse_err(&format!("{base}typo = 1\n")).contains("`project.typo`: unknown key"));
        assert!(
            parse_err(&format!("{base}[sync]\non_startup = \"always\"\n"))
                .contains("`sync.on_startup`")
        );
        assert!(
            parse_err(&format!("{base}[repositories]\ncran = \"https://x.org\"\n"))
                .contains("reserved")
        );
    }

    #[test]
    fn validates_dependency_tables() {
        let err = |deps: &str| parse_err(&with_deps(deps));
        assert!(err("x = \">= one\"").contains("`dependencies.x`: invalid version constraint"));
        assert!(
            err("x = { repo = \"mv\", github = \"a/b\" }").contains("either `repo` or `github`")
        );
        assert!(err("x = { }").contains("expected `version`, `repo` or `github`"));
        assert!(
            err("x = { version = \"1.0\", github = \"a/b\" }")
                .contains("pin a version with `tag` or `rev`")
        );
        assert!(err("x = { branch = \"dev\" }").contains("only be used with `github`"));
        assert!(err("x = { version = \"> one\" }").contains("`dependencies.x.version`"));
        assert!(Manifest::parse(&with_deps("x = { env = { A = \"1\" } }")).is_ok());
        assert!(err("x = { repo = \"nope\" }").contains("not declared in [repositories]"));
        assert!(
            err("x = { repo = \"mv\", branch = \"dev\" }").contains("only be used with `github`")
        );
        assert!(err("x = { github = \"nouser\" }").contains("expected `owner/repo`"));
        assert!(
            err("x = { github = \"a/b\", tag = \"v1\", rev = \"abc\" }").contains("only one of")
        );
        assert!(
            err("x = { github = \"a/b\", tag = \"v1\", track = true }")
                .contains("only works with a branch")
        );
        assert!(err("x = { github = \"a/b\", branc = \"dev\" }").contains("unknown key"));
        assert!(err("\"1bad\" = \"*\"").contains("not a valid R package name"));
        assert!(
            Manifest::parse(&with_deps(
                "x = { github = \"a/b\", branch = \"dev\", track = true }"
            ))
            .is_ok()
        );
    }

    #[test]
    fn edits_preserve_comments_and_layout() {
        let (mut doc, _) = ManifestDocument::parse(EXAMPLE).unwrap();
        doc.set_dependency("fixest", &DependencySpec::cran("< 0.14".parse().unwrap()));
        doc.set_dependency("data.table", &DependencySpec::cran(Constraint::any()));
        assert!(doc.remove_dependency("sf"));
        assert!(!doc.remove_dependency("sf"));
        doc.set_snapshot("2027-01-31");
        let out = doc.to_string();
        assert!(out.starts_with("# My analysis\n"));
        assert!(out.contains("fixest = \"< 0.14\"   # keep the old API\n"));
        assert!(
            out.contains("polars = { repo = \"multiverse\", env = { NOT_CRAN = \"true\" } }\n")
        );
        assert!(!out.contains("sf = "));
        assert!(out.contains("snapshot = \"2027-01-31\"\n"));
        // The new dependency goes at the end of [dependencies], before [sync].
        let deps_end = out.find("[sync]").unwrap();
        // A key with a dot must be quoted in TOML.
        let dt = out.find("\"data.table\" = \"*\"").unwrap();
        assert!(dt < deps_end && dt > out.find("polars =").unwrap());
        // The result is still a valid manifest.
        let m = Manifest::parse(&out).unwrap();
        assert_eq!(m.project.snapshot, "2027-01-31");
        assert_eq!(m.dependencies.len(), 8);
    }

    #[test]
    fn writes_github_and_repository_specs() {
        let (mut doc, _) = ManifestDocument::parse(&with_deps("")).unwrap();
        doc.set_dependency(
            "pkgA",
            &DependencySpec {
                source: DependencySource::GitHub {
                    owner: "user".into(),
                    repo: "pkgA".into(),
                    reference: GitRef::Branch("dev".into()),
                    track: true,
                },
                env: BTreeMap::new(),
            },
        );
        doc.set_r(&"4.5".parse().unwrap());
        let out = doc.to_string();
        assert!(out.contains("pkgA = { github = \"user/pkgA\", branch = \"dev\", track = true }"));
        assert!(out.contains("r = \"4.5\""));
        assert!(Manifest::parse(&out).is_ok());
    }

    #[test]
    fn writes_version_tables() {
        let (mut doc, _) = ManifestDocument::parse(&with_deps("")).unwrap();
        doc.set_dependency(
            "tidypolars",
            &DependencySpec {
                source: DependencySource::Repository {
                    alias: "mv".into(),
                    constraint: ">= 0.10".parse().unwrap(),
                },
                env: BTreeMap::new(),
            },
        );
        doc.set_dependency(
            "arrow",
            &DependencySpec {
                source: DependencySource::Cran {
                    constraint: Constraint::any(),
                },
                env: BTreeMap::from([("LIBARROW_BINARY".into(), "true".into())]),
            },
        );
        let out = doc.to_string();
        assert!(out.contains("tidypolars = { repo = \"mv\", version = \">= 0.10\" }"));
        assert!(out.contains("arrow = { env = { LIBARROW_BINARY = \"true\" } }"));
        let m = Manifest::parse(&out).unwrap();
        assert_eq!(m.dependency("arrow").unwrap().env.len(), 1);
    }

    #[test]
    fn creates_dependencies_table_when_missing() {
        let text = "[project]\nname = \"p\"\nr = \"4.4\"\nsnapshot = \"2026-10-04\"\n";
        let (mut doc, _) = ManifestDocument::parse(text).unwrap();
        doc.set_dependency("fixest", &DependencySpec::cran(Constraint::any()));
        let m = Manifest::parse(&doc.to_string()).unwrap();
        assert_eq!(m.dependencies.len(), 1);
    }

    #[test]
    fn validates_dates() {
        assert!(is_valid_date("2024-02-29"));
        assert!(!is_valid_date("2023-02-29"));
        assert!(!is_valid_date("2026-13-01"));
        assert!(!is_valid_date("2026-1-01"));
        assert!(!is_valid_date("26-10-04xx"));
    }
}
