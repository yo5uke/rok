//! `rok status`: what is out of sync, worst first, without using the network
//! (requirements, chapter 5 "status").
//!
//! Levels follow the requirements' table: ✖ for an R minor version that does not match (or is
//! not installed), ! for the manifest, lockfile and library disagreeing, ℹ for information
//! (patch versions, mixed snapshot dates, packages rok does not manage). System libraries (4)
//! and scanner suggestions (5) are added in later steps.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::dcf;
use crate::install;
use crate::lockfile::{Lockfile, Source, name_order};
use crate::manifest::Manifest;
use crate::ops::{self, manifest_copy};
use crate::rdetect::RInstallation;
use crate::syslibs::AptAdvice;
use crate::version::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
    Info,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Info => "info",
        }
    }
}

/// One finding, with the next step to take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub level: Level,
    /// A stable identifier for programs, such as `library-mismatch`.
    pub code: &'static str,
    pub message: String,
    pub details: Vec<String>,
    pub fix: Option<String>,
}

/// An entry of a project library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// A link into rok's cache. `version` is `None` when the link is broken; `key` is the
    /// cache key of the linked build (see `install::Context`).
    Linked {
        version: Option<Version>,
        key: String,
    },
    /// A directory that rok did not create (for example from `install.packages()`).
    Other { version: Option<Version> },
}

impl Installed {
    pub fn version(&self) -> Option<&Version> {
        match self {
            Installed::Linked { version, .. } | Installed::Other { version } => version.as_ref(),
        }
    }
}

/// Reads a project library: each entry and its version (from DESCRIPTION).
pub fn read_library(library: &Path, cache_root: &Path) -> BTreeMap<String, Installed> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(library).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !dcf::is_package_name(&name) {
            continue;
        }
        let path = entry.path();
        let version = std::fs::read_to_string(path.join("DESCRIPTION"))
            .ok()
            .and_then(|t| dcf::parse(&t).ok())
            .and_then(|r| {
                r.first()
                    .and_then(|r| r.get("Version"))
                    .and_then(|v| v.parse().ok())
            });
        let target = std::fs::read_link(&path)
            .ok()
            .filter(|t| t.starts_with(cache_root));
        out.insert(
            name,
            match target {
                Some(t) => Installed::Linked {
                    version,
                    key: t
                        .parent()
                        .and_then(|k| k.file_name())
                        .map(|k| k.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                },
                None => Installed::Other { version },
            },
        );
    }
    out
}

/// What [`check`] looks at.
pub struct Inputs<'a> {
    pub manifest: &'a Manifest,
    pub lock: Option<&'a Lockfile>,
    /// The installed R used for the project (`None` if no installed R has its minor version).
    pub r: Option<&'a RInstallation>,
    /// The project library's entries.
    pub library: &'a BTreeMap<String, Installed>,
    /// Missing system libraries (Linux), if checked.
    pub system: Option<&'a SystemCheck>,
    /// What the code scan found, if it ran.
    pub scan: Option<&'a ops::ScanFindings>,
}

/// Missing shared libraries on Linux (requirements, chapters 6 and 7).
#[derive(Debug, Clone, Default)]
pub struct SystemCheck {
    /// Libraries R itself needs.
    pub r_missing: Vec<String>,
    /// Libraries the library's packages need, by package.
    pub packages: BTreeMap<String, Vec<String>>,
    /// How to install the packages' libraries.
    pub advice: AptAdvice,
    /// How to install R's libraries.
    pub r_advice: AptAdvice,
}

/// Finds problems, sorted from the most to the least serious.
pub fn check(inputs: &Inputs) -> Vec<Problem> {
    let Inputs {
        manifest,
        lock,
        r,
        library,
        system,
        scan,
    } = *inputs;
    let mut problems = Vec::new();
    let (minor, _) = ops::manifest_r(manifest);

    // 1. R.
    if r.is_none() {
        problems.push(Problem {
            level: Level::Error,
            code: "r-missing",
            message: format!("This project needs R {minor}, which is not installed."),
            details: Vec::new(),
            fix: Some(format!(
                "Run `rok r install` to install R {minor}, or `rok r pin` to move the project to an installed R."
            )),
        });
    }
    if let (Some(sys), Some(r)) = (system, r)
        && !sys.r_missing.is_empty()
    {
        let n = sys.r_missing.len();
        problems.push(Problem {
            level: Level::Error,
            code: "r-libraries",
            message: format!(
                "R {} cannot start: {n} shared librar{} it needs {} missing.",
                r.version,
                if n == 1 { "y" } else { "ies" },
                if n == 1 { "is" } else { "are" }
            ),
            details: sys.r_missing.clone(),
            fix: Some(match sys.r_advice.command() {
                Some(cmd) if sys.r_advice.unknown.is_empty() => format!("Run `{cmd}`."),
                _ => "Install the system packages that provide them (`apt-file search <library>` finds them)."
                    .to_string(),
            }),
        });
    }

    // 2. Manifest and lockfile.
    match lock {
        None => problems.push(Problem {
            level: Level::Warning,
            code: "lock-missing",
            message: "rok.lock is missing.".to_string(),
            details: Vec::new(),
            fix: Some("Run `rok sync` to create it.".to_string()),
        }),
        Some(l) if !ops::lock_is_current(manifest, l) => problems.push(Problem {
            level: Level::Warning,
            code: "lock-outdated",
            message: "rok.toml has changed since rok.lock was written.".to_string(),
            details: manifest_differences(manifest, l),
            fix: Some("Run `rok sync` to update rok.lock and the library.".to_string()),
        }),
        Some(_) => {}
    }

    // 3. Lockfile and library.
    if let Some(l) = lock {
        let mut details = Vec::new();
        for p in &l.packages {
            match library.get(&p.name) {
                None => details.push(format!("{} {} is not installed", p.name, p.version)),
                Some(Installed::Linked { version: None, .. }) => details.push(format!(
                    "{} {}: the link to the cache is broken",
                    p.name, p.version
                )),
                Some(i) => {
                    if let Some(v) = i.version().filter(|v| *v != &p.version) {
                        details.push(format!("{} is {v}, but rok.lock has {}", p.name, p.version));
                    } else if let (Source::GitHub { commit, .. }, Installed::Linked { key, .. }) =
                        (&p.source, i)
                        && !key.ends_with(&install::github_key_suffix(commit))
                    {
                        details.push(format!(
                            "{} {} is not built from the commit rok.lock records ({})",
                            p.name,
                            p.version,
                            &commit[..7]
                        ));
                    }
                }
            }
        }
        for (name, i) in library {
            if matches!(i, Installed::Linked { .. }) && l.package(name).is_none() {
                details.push(format!("{name} is installed but not in rok.lock"));
            }
        }
        if !details.is_empty() {
            problems.push(Problem {
                level: Level::Warning,
                code: "library-mismatch",
                message: format!(
                    "The library does not match rok.lock ({} difference{}).",
                    details.len(),
                    ops::plural(details.len())
                ),
                details,
                fix: Some("Run `rok sync`.".to_string()),
            });
        }
    }

    // 4. System libraries.
    if let Some(sys) = system
        && !sys.packages.is_empty()
    {
        let n = sys.packages.len();
        let mut details: Vec<String> = sys
            .packages
            .iter()
            .map(|(name, libs)| format!("{name}: {}", libs.join(", ")))
            .collect();
        if !sys.advice.unknown.is_empty() {
            details.push(format!(
                "No system package was found for: {}",
                sys.advice.unknown.join(", ")
            ));
        }
        let fix = sys.advice.command().map(|c| {
            if sys.advice.broad {
                format!(
                    "Run `{c}`. These are the development packages P3M lists, which include more than is needed{}.",
                    if sys.advice.needs_update {
                        " (apt's package lists are missing, so the exact ones cannot be found)"
                    } else {
                        ""
                    }
                )
            } else {
                format!("Run `{c}`.")
            }
        });
        problems.push(Problem {
            level: Level::Warning,
            code: "system-libraries",
            message: format!(
                "{n} package{} need{} system libraries that are not installed.",
                ops::plural(n),
                if n == 1 { "s" } else { "" }
            ),
            details,
            fix,
        });
    }

    // 5. Suggestions from the code.
    if let Some(f) = scan {
        if !f.undeclared.is_empty() {
            let n = f.undeclared.len();
            let names: Vec<&str> = f.undeclared.iter().map(|(n, _)| n.as_str()).collect();
            problems.push(Problem {
                level: Level::Info,
                code: "undeclared",
                message: format!(
                    "{n} package{} the code uses {} not declared in rok.toml.",
                    ops::plural(n),
                    if n == 1 { "is" } else { "are" }
                ),
                details: f
                    .undeclared
                    .iter()
                    .map(|(name, why)| format!("{name} ({why})"))
                    .collect(),
                fix: Some(format!(
                    "Run `rok add {}` to declare them.",
                    names.join(" ")
                )),
            });
        }
        if !f.unused.is_empty() {
            let n = f.unused.len();
            problems.push(Problem {
                level: Level::Info,
                code: "unused",
                message: format!(
                    "{n} declared package{} {} not used in the code.",
                    ops::plural(n),
                    if n == 1 { "is" } else { "are" }
                ),
                details: f.unused.clone(),
                fix: Some(format!(
                    "If they are not needed, run `rok remove {}`.",
                    f.unused.join(" ")
                )),
            });
        }
    }

    // 6. Information.
    if let (Some(l), Some(r)) = (lock, r)
        && l.r.minor() == r.version.minor()
        && l.r != r.version
    {
        problems.push(Problem {
            level: Level::Info,
            code: "r-patch",
            message: format!("Using R {}; rok.lock records R {}.", r.version, l.r),
            details: Vec::new(),
            fix: None,
        });
    }
    if let Some(l) = lock {
        let rebuilt = ops::rebuilt_packages(l);
        if !rebuilt.is_empty() {
            problems.push(Problem {
                level: Level::Info,
                code: "rebuilt-from-git",
                message: format!(
                    "{} package{} {} rebuilt from Git because the repository no longer has the release; the result may differ from it.",
                    rebuilt.len(),
                    ops::plural(rebuilt.len()),
                    if rebuilt.len() == 1 { "was" } else { "were" }
                ),
                details: rebuilt,
                fix: None,
            });
        }
    }
    if let Some(l) = lock {
        let mixed: Vec<String> = l
            .packages
            .iter()
            .filter_map(|p| {
                p.source
                    .snapshot()
                    .filter(|d| *d != l.snapshot.date)
                    .map(|d| format!("{} {} ({d})", p.name, p.version))
            })
            .collect();
        if !mixed.is_empty() {
            problems.push(Problem {
                level: Level::Info,
                code: "mixed-dates",
                message: format!(
                    "{} package{} come{} from a snapshot other than the project's ({}).",
                    mixed.len(),
                    ops::plural(mixed.len()),
                    if mixed.len() == 1 { "s" } else { "" },
                    l.snapshot.date
                ),
                details: mixed,
                fix: None,
            });
        }
    }
    let in_lock = |n: &str| lock.is_some_and(|l| l.package(n).is_some());
    let (unmanaged, outside): (Vec<String>, Vec<String>) = library
        .iter()
        .filter(|(n, i)| matches!(i, Installed::Other { .. }) && !in_lock(n))
        .map(|(n, i)| match i.version() {
            Some(v) => format!("{n} {v}"),
            None => n.clone(),
        })
        .partition(|label| {
            manifest
                .unmanaged
                .iter()
                .any(|u| label.split(' ').next() == Some(u.as_str()))
        });
    if !unmanaged.is_empty() {
        problems.push(Problem {
            level: Level::Info,
            code: "unmanaged",
            message: format!(
                "{} unmanaged package{} (listed in [unmanaged]).",
                unmanaged.len(),
                ops::plural(unmanaged.len())
            ),
            details: unmanaged,
            fix: None,
        });
    }
    if !outside.is_empty() {
        problems.push(Problem {
            level: Level::Info,
            code: "installed-outside",
            message: format!(
                "{} package{} in the library {} not managed by rok.",
                outside.len(),
                ops::plural(outside.len()),
                if outside.len() == 1 { "is" } else { "are" }
            ),
            details: outside,
            fix: Some(
                "Add them with `rok add`, or list them in [unmanaged] in rok.toml.".to_string(),
            ),
        });
    }

    problems.sort_by_key(|p| p.level);
    problems
}

/// How the manifest differs from the copy in the lockfile, in words.
fn manifest_differences(manifest: &Manifest, lock: &Lockfile) -> Vec<String> {
    let mut out = Vec::new();
    if manifest.project.snapshot != lock.snapshot.date {
        out.push(format!(
            "snapshot: rok.toml has {}, rok.lock has {}",
            manifest.project.snapshot, lock.snapshot.date
        ));
    }
    let (minor, pinned) = ops::manifest_r(manifest);
    if lock.r.minor() != minor || pinned.is_some_and(|p| p != &lock.r) {
        out.push(format!(
            "R: rok.toml has {}, rok.lock has {}",
            manifest.project.r, lock.r
        ));
    }
    let now = manifest_copy(manifest);
    let mut added: Vec<&String> = now
        .dependencies
        .iter()
        .filter(|d| !lock.manifest.dependencies.contains(d))
        .collect();
    let mut removed: Vec<&String> = lock
        .manifest
        .dependencies
        .iter()
        .filter(|d| !now.dependencies.contains(d))
        .collect();
    added.sort_by(|a, b| name_order(a, b));
    removed.sort_by(|a, b| name_order(a, b));
    out.extend(added.iter().map(|d| format!("{d} was added")));
    out.extend(removed.iter().map(|d| format!("{d} was removed")));
    for (name, c) in &now.constraints {
        if lock.manifest.constraints.get(name) != Some(c) {
            out.push(format!("{name}: the version constraint changed to {c}"));
        }
    }
    for name in lock.manifest.constraints.keys() {
        if !now.constraints.contains_key(name) && now.dependencies.contains(name) {
            out.push(format!("{name}: the version constraint was removed"));
        }
    }
    let kept = |n: &String| lock.manifest.dependencies.contains(n);
    for (name, key) in &now.sources {
        if kept(name) && lock.manifest.sources.get(name) != Some(key) {
            out.push(format!("{name}: the source changed to `{key}`"));
        }
    }
    for name in lock.manifest.sources.keys() {
        if !now.sources.contains_key(name) && now.dependencies.contains(name) {
            out.push(format!("{name}: the source changed to CRAN"));
        }
    }
    let mut urls = BTreeSet::new();
    for p in &lock.packages {
        if let Some(d) = manifest.dependency(&p.name)
            && kept(&p.name)
            && d.env != p.env
        {
            out.push(format!(
                "{}: the build environment variables changed",
                p.name
            ));
        }
        if let Source::Repository {
            repository,
            url: Some(url),
            ..
        } = &p.source
            && let Some(now) = manifest.repositories.get(repository)
            && now != url
        {
            urls.insert(format!(
                "[repositories] {repository}: the URL changed to {now}"
            ));
        }
    }
    out.extend(urls);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdetect::RKind;
    use std::path::PathBuf;

    fn manifest(deps: &str) -> Manifest {
        Manifest::parse(&format!(
            "[project]\nname = \"p\"\nr = \"4.6\"\nsnapshot = \"2026-10-01\"\n[dependencies]\n{deps}\n[unmanaged]\npackages = [\"limma\"]\n"
        ))
        .unwrap()
    }

    fn lock() -> Lockfile {
        Lockfile::parse(
            "version = 1\ngenerated-by = \"rok\"\n[r]\nversion = \"4.6.1\"\n[snapshot]\ndate = \"2026-10-01\"\nrepository = \"x\"\n\
             [manifest]\ndependencies = [\"fixest\"]\n\
             [[package]]\nname = \"fixest\"\nversion = \"0.12.1\"\nsource = { repository = \"cran\", snapshot = \"2024-06-14\" }\ndependencies = [\"Rcpp\"]\n\
             [[package]]\nname = \"Rcpp\"\nversion = \"1.1.2\"\nsource = { repository = \"cran\", snapshot = \"2026-10-01\" }\n",
        )
        .unwrap()
    }

    fn r(v: &str) -> RInstallation {
        RInstallation {
            version: v.parse().unwrap(),
            r_home: PathBuf::from("/r"),
            executable: PathBuf::from("/r/bin/R"),
            kind: RKind::Path,
        }
    }

    fn codes(problems: &[Problem]) -> Vec<&str> {
        problems.iter().map(|p| p.code).collect()
    }

    #[test]
    fn reports_a_project_in_sync_as_fine_apart_from_information() {
        let lib = BTreeMap::from([
            (
                "fixest".to_string(),
                Installed::Linked {
                    version: Some("0.12.1".parse().unwrap()),
                    key: "k".into(),
                },
            ),
            (
                "Rcpp".to_string(),
                Installed::Linked {
                    version: Some("1.1.2".parse().unwrap()),
                    key: "k".into(),
                },
            ),
        ]);
        let (m, l, r) = (manifest("fixest = \"*\""), lock(), r("4.6.1"));
        let problems = check(&Inputs {
            manifest: &m,
            lock: Some(&l),
            r: Some(&r),
            library: &lib,
            system: None,
            scan: None,
        });
        assert_eq!(codes(&problems), ["mixed-dates"]);
        assert_eq!(problems[0].details, ["fixest 0.12.1 (2024-06-14)"]);
    }

    #[test]
    fn checks_github_packages_by_commit() {
        let commit = "a".repeat(40);
        let l = Lockfile::parse(&format!(
            "version = 1\ngenerated-by = \"rok\"\n[r]\nversion = \"4.6.1\"\n[snapshot]\ndate = \"2026-10-01\"\nrepository = \"x\"\n\
             [manifest]\ndependencies = [\"praise\"]\nsources = {{ praise = \"github:o/praise\" }}\n\
             [[package]]\nname = \"praise\"\nversion = \"1.0.0\"\nsource = {{ github = \"o/praise\", commit = \"{commit}\" }}\n"
        ))
        .unwrap();
        let linked = |key: &str| {
            BTreeMap::from([(
                "praise".to_string(),
                Installed::Linked {
                    version: Some("1.0.0".parse().unwrap()),
                    key: key.into(),
                },
            )])
        };
        let (m, r) = (manifest("praise = { github = \"o/praise\" }"), r("4.6.1"));
        let run = |lib: &BTreeMap<String, Installed>, m: &Manifest| {
            check(&Inputs {
                manifest: m,
                lock: Some(&l),
                r: Some(&r),
                library: lib,
                system: None,
                scan: None,
            })
        };
        assert!(run(&linked("4.6-x-source-gh-aaaaaaaaaaaa"), &m).is_empty());
        let other = run(&linked("4.6-x-source-gh-bbbbbbbbbbbb"), &m);
        assert_eq!(codes(&other), ["library-mismatch"]);
        assert_eq!(
            other[0].details,
            ["praise 1.0.0 is not built from the commit rok.lock records (aaaaaaa)"]
        );
        let tagged = manifest("praise = { github = \"o/praise\", tag = \"v1\" }");
        let changed = run(&linked("4.6-x-source-gh-aaaaaaaaaaaa"), &tagged);
        assert_eq!(codes(&changed), ["lock-outdated"]);
        assert_eq!(
            changed[0].details,
            ["praise: the source changed to `github:o/praise#tag=v1`"]
        );
    }

    #[test]
    fn reports_missing_system_libraries() {
        let lib = BTreeMap::from([
            (
                "fixest".to_string(),
                Installed::Linked {
                    version: Some("0.12.1".parse().unwrap()),
                    key: "k".into(),
                },
            ),
            (
                "Rcpp".to_string(),
                Installed::Linked {
                    version: Some("1.1.2".parse().unwrap()),
                    key: "k".into(),
                },
            ),
        ]);
        let sys = SystemCheck {
            r_missing: vec!["libblas.so.3".into()],
            packages: BTreeMap::from([(
                "sf".to_string(),
                vec!["libgdal.so.34".into(), "libproj.so.25".into()],
            )]),
            advice: AptAdvice {
                packages: vec!["libgdal34t64".into(), "libproj25".into()],
                ..Default::default()
            },
            r_advice: AptAdvice::default(),
        };
        let (m, l, r) = (manifest("fixest = \"*\""), lock(), r("4.6.1"));
        let problems = check(&Inputs {
            manifest: &m,
            lock: Some(&l),
            r: Some(&r),
            library: &lib,
            system: Some(&sys),
            scan: None,
        });
        assert_eq!(
            codes(&problems),
            ["r-libraries", "system-libraries", "mixed-dates"]
        );
        assert_eq!(problems[1].details, ["sf: libgdal.so.34, libproj.so.25"]);
        assert_eq!(
            problems[1].fix.as_deref(),
            Some("Run `sudo apt-get install -y libgdal34t64 libproj25`.")
        );
    }

    #[test]
    fn reports_scan_findings_as_information() {
        let lib = BTreeMap::from([
            (
                "fixest".to_string(),
                Installed::Linked {
                    version: Some("0.12.1".parse().unwrap()),
                    key: "k".into(),
                },
            ),
            (
                "Rcpp".to_string(),
                Installed::Linked {
                    version: Some("1.1.2".parse().unwrap()),
                    key: "k".into(),
                },
            ),
        ]);
        let findings = ops::ScanFindings {
            undeclared: vec![("sf".into(), "for geom_sf() in a.R:3".into())],
            unused: vec!["fixest".into()],
        };
        let (m, l, r) = (manifest("fixest = \"*\""), lock(), r("4.6.1"));
        let problems = check(&Inputs {
            manifest: &m,
            lock: Some(&l),
            r: Some(&r),
            library: &lib,
            system: None,
            scan: Some(&findings),
        });
        assert_eq!(codes(&problems), ["undeclared", "unused", "mixed-dates"]);
        assert!(problems.iter().all(|p| p.level == Level::Info));
        assert_eq!(problems[0].details, ["sf (for geom_sf() in a.R:3)"]);
        assert_eq!(
            problems[0].fix.as_deref(),
            Some("Run `rok add sf` to declare them.")
        );
    }

    #[test]
    fn orders_problems_by_severity() {
        let lib = BTreeMap::from([
            (
                "fixest".to_string(),
                Installed::Linked {
                    version: None,
                    key: "k".into(),
                },
            ),
            (
                "old".to_string(),
                Installed::Linked {
                    version: Some("1.0".parse().unwrap()),
                    key: "k".into(),
                },
            ),
            (
                "limma".to_string(),
                Installed::Other {
                    version: Some("3.60.0".parse().unwrap()),
                },
            ),
            ("mine".to_string(), Installed::Other { version: None }),
        ]);
        let (m, l) = (manifest("fixest = \"*\"\nsf = \"*\""), lock());
        let problems = check(&Inputs {
            manifest: &m,
            lock: Some(&l),
            r: None,
            library: &lib,
            system: None,
            scan: None,
        });
        assert_eq!(
            codes(&problems),
            [
                "r-missing",
                "lock-outdated",
                "library-mismatch",
                "mixed-dates",
                "unmanaged",
                "installed-outside"
            ]
        );
        assert_eq!(problems[1].details, ["sf was added"]);
        assert_eq!(
            problems[2].details,
            [
                "fixest 0.12.1: the link to the cache is broken",
                "Rcpp 1.1.2 is not installed",
                "old is installed but not in rok.lock"
            ]
        );
        assert_eq!(problems[4].details, ["limma 3.60.0"]);
        assert_eq!(problems[5].details, ["mine"]);
    }

    #[test]
    fn notes_a_different_r_patch_version() {
        let lib = BTreeMap::new();
        let (m, l, r) = (manifest("fixest = \"*\""), lock(), r("4.6.0"));
        let problems = check(&Inputs {
            manifest: &m,
            lock: Some(&l),
            r: Some(&r),
            library: &lib,
            system: None,
            scan: None,
        });
        assert!(problems.iter().any(
            |p| p.code == "r-patch" && p.message == "Using R 4.6.0; rok.lock records R 4.6.1."
        ));
    }

    #[cfg(unix)]
    #[test]
    fn reads_library_entries() {
        let t = tempfile::tempdir().unwrap();
        let (cache, lib) = (t.path().join("cache"), t.path().join("lib"));
        let pkg = cache.join("R6/2.6.1/k/R6");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("DESCRIPTION"), "Package: R6\nVersion: 2.6.1\n").unwrap();
        std::fs::create_dir_all(lib.join("mine")).unwrap();
        std::os::unix::fs::symlink(&pkg, lib.join("R6")).unwrap();
        std::os::unix::fs::symlink(cache.join("gone/1/k/gone"), lib.join("gone")).unwrap();
        let entries = read_library(&lib, &cache);
        assert_eq!(
            entries["R6"],
            Installed::Linked {
                version: Some("2.6.1".parse().unwrap()),
                key: "k".into(),
            }
        );
        assert_eq!(
            entries["gone"],
            Installed::Linked {
                version: None,
                key: "k".into()
            }
        );
        assert_eq!(entries["mine"], Installed::Other { version: None });
    }
}
