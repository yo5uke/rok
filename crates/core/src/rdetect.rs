//! Finding R installations without starting R.
//!
//! An installation is identified by its R_HOME, the directory that contains
//! `library/base/DESCRIPTION`. The version is read from that file, which is much faster than
//! running `R --version`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::dcf;
use crate::paths::UserDirs;
use crate::version::Version;

/// Where an installation was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RKind {
    /// Installed by rok (see [`UserDirs::r`]).
    Managed,
    /// Found on `PATH`.
    Path,
    /// A conventional location such as `/opt/R/<version>`, `/usr/lib/R` or, on Windows,
    /// `%ProgramFiles%/R/R-<version>`.
    System,
}

/// The file rok puts in each R it installs on Windows, where its directory
/// (`%LOCALAPPDATA%/Programs/R`) also holds R installed by CRAN's installer.
pub const MANAGED_MARK: &str = ".rok-installed";

/// Whether the R in `dir` (a directory of [`UserDirs::r`]) was installed by rok.
fn is_managed(dir: &Path) -> bool {
    !cfg!(windows) || dir.join(MANAGED_MARK).is_file()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RInstallation {
    pub version: Version,
    pub r_home: PathBuf,
    /// The `R` executable that was found (for `PATH`, the one on `PATH`).
    pub executable: PathBuf,
    pub kind: RKind,
}

impl RInstallation {
    /// `Rscript` in this installation.
    pub fn rscript(&self) -> PathBuf {
        self.r_home.join("bin").join(exe_name("Rscript"))
    }
}

/// The file name of an executable (`R.exe` on Windows).
pub fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Reads the R version from `<r_home>/library/base/DESCRIPTION`.
pub fn r_home_version(r_home: &Path) -> Option<Version> {
    let text = std::fs::read_to_string(r_home.join("library/base/DESCRIPTION")).ok()?;
    let records = dcf::parse(&text).ok()?;
    records.first()?.get("Version")?.parse().ok()
}

fn is_r_home(dir: &Path) -> bool {
    dir.join("library/base/DESCRIPTION").is_file()
}

/// Finds R_HOME for an `R` executable.
///
/// Tries the usual layouts first (`<prefix>/bin/R` with R_HOME at `<prefix>/lib/R`,
/// `<prefix>/lib64/R` or `<prefix>` itself, and `R_HOME/bin/x64/R.exe` on Windows), then the
/// `R_HOME_DIR=` line of the shell script that Unix builds install as `bin/R`.
pub fn r_home_of(executable: &Path) -> Option<PathBuf> {
    let exe = crate::fsutil::canonicalize(executable).ok()?;
    let bin = exe.parent()?;
    let prefix = bin.parent()?;
    let mut candidates = vec![
        prefix.join("lib/R"),
        prefix.join("lib64/R"),
        prefix.to_path_buf(),
    ];
    if let Some(above) = prefix.parent() {
        candidates.push(above.to_path_buf()); // R_HOME/bin/x64/R.exe
    }
    if let Some(home) = candidates.into_iter().find(|c| is_r_home(c)) {
        return Some(home);
    }
    let script = std::fs::read(&exe).ok()?;
    let script = String::from_utf8_lossy(&script);
    script.lines().find_map(|line| {
        let value = line.trim().strip_prefix("R_HOME_DIR=")?;
        let home = PathBuf::from(value.trim_matches(|c| c == '"' || c == '\''));
        (home.is_absolute() && is_r_home(&home)).then_some(home)
    })
}

/// Finds R installations: managed by rok, on `PATH`, and in conventional locations.
///
/// Each R_HOME is reported once, with the first kind it was found as (managed, then `PATH`,
/// then system). Within a kind, installations keep the order they were found in.
pub fn find_installations(dirs: &UserDirs, path_var: Option<OsString>) -> Vec<RInstallation> {
    let r = exe_name("R");
    let mut found: Vec<(PathBuf, RKind)> = Vec::new();

    let mut managed: Vec<PathBuf> = std::fs::read_dir(dirs.r_installs())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    managed.sort();
    found.extend(managed.into_iter().map(|d| {
        let kind = if is_managed(&d) {
            RKind::Managed
        } else {
            RKind::System
        };
        (d.join("bin").join(&r), kind)
    }));

    if let Some(path) = path_var {
        found.extend(std::env::split_paths(&path).map(|d| (d.join(&r), RKind::Path)));
    }

    if cfg!(windows) {
        // CRAN's installer: for all users, or for one user (`/CURRENTUSER`).
        let roots = [
            std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("R")),
            std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("Programs").join("R")),
        ];
        for root in roots.into_iter().flatten() {
            let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path().join("bin").join(&r))
                .collect();
            dirs.sort();
            found.extend(dirs.into_iter().map(|p| (p, RKind::System)));
        }
    } else {
        let mut opt: Vec<PathBuf> = std::fs::read_dir("/opt/R")
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("bin/R"))
            .collect();
        opt.sort();
        found.extend(opt.into_iter().map(|p| (p, RKind::System)));
        for home in ["/usr/lib/R", "/usr/lib64/R", "/usr/local/lib/R"] {
            found.push((PathBuf::from(home).join("bin/R"), RKind::System));
        }
    }

    let mut result: Vec<RInstallation> = Vec::new();
    for (executable, kind) in found {
        if !executable.is_file() {
            continue;
        }
        let Some(r_home) = r_home_of(&executable) else {
            continue;
        };
        if result.iter().any(|i| i.r_home == r_home) {
            continue;
        }
        if let Some(version) = r_home_version(&r_home) {
            result.push(RInstallation {
                version,
                r_home,
                executable,
                kind,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::canonicalize;
    use std::fs;

    fn base_description(home: &Path, version: &str) {
        fs::create_dir_all(home.join("library/base")).unwrap();
        fs::write(
            home.join("library/base/DESCRIPTION"),
            format!("Package: base\nVersion: {version}\nBuilt: R {version}; ; 2026-01-01; unix\n"),
        )
        .unwrap();
    }

    /// Creates a fake R installation with `bin/R` at `<prefix>/bin/R` and R_HOME at `home`.
    fn fake_r(prefix: &Path, home: &Path, version: &str, script: &str) {
        base_description(home, version);
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::write(prefix.join("bin/R"), script).unwrap();
    }

    /// A fake R in this OS's layout: `<dir>/bin/R` with R_HOME `<dir>/lib/R` (Posit's builds),
    /// or on Windows `<dir>/bin/R.exe` with R_HOME `<dir>`.
    fn fake_install(dir: &Path, version: &str) {
        if cfg!(windows) {
            base_description(dir, version);
            fs::create_dir_all(dir.join("bin")).unwrap();
            fs::write(dir.join("bin/R.exe"), "").unwrap();
        } else {
            fake_r(dir, &dir.join("lib/R"), version, "#!/bin/sh\n");
        }
    }

    fn dirs(root: &Path) -> UserDirs {
        UserDirs::under(root)
    }

    #[cfg(unix)]
    #[test]
    fn reads_version_and_r_home_from_layouts() {
        let t = tempfile::tempdir().unwrap();
        let root = canonicalize(t.path()).unwrap();
        // Posit layout: <prefix>/bin/R, R_HOME = <prefix>/lib/R
        let posit = root.join("posit/4.4.2");
        fake_r(&posit, &posit.join("lib/R"), "4.4.2", "#!/bin/sh\n");
        assert_eq!(r_home_of(&posit.join("bin/R")), Some(posit.join("lib/R")));
        assert_eq!(
            r_home_version(&posit.join("lib/R")).unwrap().as_str(),
            "4.4.2"
        );
        // Debian layout: /usr/bin/R is a script naming R_HOME_DIR elsewhere.
        let home = root.join("usr/share/R");
        let usr = root.join("usr");
        fake_r(
            &usr,
            &home,
            "4.6.1",
            &format!(
                "#!/bin/bash\n# Shell wrapper\nR_HOME_DIR=\"{}\"\n",
                home.display()
            ),
        );
        assert_eq!(r_home_of(&usr.join("bin/R")), Some(home));
        // Not R at all.
        let other = root.join("other");
        fs::create_dir_all(other.join("bin")).unwrap();
        fs::write(other.join("bin/R"), "#!/bin/sh\n").unwrap();
        assert_eq!(r_home_of(&other.join("bin/R")), None);
    }

    #[test]
    fn reads_windows_layouts() {
        let t = tempfile::tempdir().unwrap();
        let home = canonicalize(t.path()).unwrap().join("R-4.6.1");
        base_description(&home, "4.6.1");
        fs::create_dir_all(home.join("bin/x64")).unwrap();
        fs::write(home.join("bin/R.exe"), "").unwrap();
        fs::write(home.join("bin/x64/R.exe"), "").unwrap();
        assert_eq!(r_home_of(&home.join("bin/R.exe")), Some(home.clone()));
        assert_eq!(r_home_of(&home.join("bin/x64/R.exe")), Some(home));
    }

    #[test]
    fn finds_managed_and_path_installations_once() {
        let t = tempfile::tempdir().unwrap();
        let root = canonicalize(t.path()).unwrap();
        let d = dirs(&root);
        let dir = |v: &str| {
            let name = if cfg!(windows) {
                format!("R-{v}")
            } else {
                v.to_string()
            };
            d.r_installs().join(name)
        };
        for v in ["4.6.1", "4.4.2"] {
            fake_install(&dir(v), v);
            if cfg!(windows) {
                fs::write(dir(v).join(MANAGED_MARK), "").unwrap();
            }
        }
        // On Windows, R from CRAN's installer in the same directory is not rok's.
        if cfg!(windows) {
            fake_install(&dir("4.3.3"), "4.3.3");
        }
        let on_path = root.join("elsewhere/4.5.0");
        fake_install(&on_path, "4.5.0");
        // PATH lists a managed installation again and a directory without R.
        let path = std::env::join_paths([
            dir("4.4.2").join("bin"),
            on_path.join("bin"),
            root.join("empty"),
        ])
        .unwrap();
        let found: Vec<(String, RKind)> = find_installations(&d, Some(path))
            .into_iter()
            .filter(|i| i.r_home.starts_with(&root))
            .map(|i| (i.version.to_string(), i.kind))
            .collect();
        let mut expected = vec![
            ("4.4.2".to_string(), RKind::Managed),
            ("4.6.1".to_string(), RKind::Managed),
            ("4.5.0".to_string(), RKind::Path),
        ];
        if cfg!(windows) {
            expected.insert(0, ("4.3.3".to_string(), RKind::System));
        }
        assert_eq!(found, expected);
    }
}
