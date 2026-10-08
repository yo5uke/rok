//! Positron's workspace settings for the project's R (requirements chapter 6, IDE settings;
//! V7). Positron reads only absolute paths and paths starting with `~`, so the settings name
//! the R that rok installed with paths from `~`: they are the same for everyone using rok, and
//! can be shared in Git.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::platform::Os;
use crate::rdetect::{RInstallation, RKind};

/// The workspace settings file, relative to the project root.
pub const SETTINGS_FILE: &str = ".vscode/settings.json";

/// The comment written above the settings rok adds.
pub const COMMENT: &str =
    "Added by rok: the project's R. Paths from ~ are the same for everyone using rok.";

/// The setting that names the workspace's default R.
pub const DEFAULT_R: &str = "positron.r.interpreters.default";

/// The settings that make Positron use `r` in a workspace. `None` unless `r` is an R that rok
/// installed in its default place (`default_r_dir`, under `home`): for any other R, the paths
/// would differ between people.
pub fn positron_settings(
    os: Os,
    r: &RInstallation,
    default_r_dir: &Path,
    home: &Path,
) -> Option<Vec<(&'static str, Value)>> {
    if r.kind != RKind::Managed {
        return None;
    }
    let binary = positron_binary(os, r);
    if !binary.starts_with(default_r_dir) {
        return None;
    }
    let binary = tilde(&binary, home)?;
    Some(match os {
        Os::Linux => vec![
            (
                "positron.r.customRootFolders",
                json!([tilde(default_r_dir, home)?]),
            ),
            (DEFAULT_R, json!(binary)),
        ],
        Os::Windows => vec![(DEFAULT_R, json!(binary))],
        Os::MacOs => vec![
            ("positron.r.customBinaries", json!([binary.clone()])),
            (DEFAULT_R, json!(binary)),
        ],
    })
}

/// The R binary Positron runs for an installation (it prefers `bin/x64/R.exe` on Windows).
pub fn positron_binary(os: Os, r: &RInstallation) -> PathBuf {
    match os {
        Os::Windows => r.r_home.join("bin").join("x64").join("R.exe"),
        Os::Linux => r.executable.clone(),
        Os::MacOs => r.r_home.join("bin").join("R"),
    }
}

/// `path` as `~/...` with `/` between parts, if it is under `home`.
fn tilde(path: &Path, home: &Path) -> Option<String> {
    let rest = path.strip_prefix(home).ok()?;
    let parts: Vec<String> = rest
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(format!("~/{}", parts.join("/")))
}

/// Whether rok runs inside Positron (its R console or terminal).
pub fn in_positron(var: impl Fn(&str) -> Option<String>) -> bool {
    var("POSITRON").is_some_and(|v| !v.is_empty())
        || var("TERM_PROGRAM").is_some_and(|v| v.eq_ignore_ascii_case("positron"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(home: &str, exe: &str, kind: RKind) -> RInstallation {
        RInstallation {
            version: "4.5.3".parse().unwrap(),
            r_home: PathBuf::from(home),
            executable: PathBuf::from(exe),
            kind,
        }
    }

    #[test]
    fn names_rok_s_r_from_home() {
        let home = Path::new("/home/me");
        let rdir = Path::new("/home/me/.local/share/R/rok/r");
        let linux = r(
            "/home/me/.local/share/R/rok/r/4.5.3/lib/R",
            "/home/me/.local/share/R/rok/r/4.5.3/bin/R",
            RKind::Managed,
        );
        assert_eq!(
            positron_settings(Os::Linux, &linux, rdir, home).unwrap(),
            vec![
                (
                    "positron.r.customRootFolders",
                    json!(["~/.local/share/R/rok/r"])
                ),
                (DEFAULT_R, json!("~/.local/share/R/rok/r/4.5.3/bin/R")),
            ]
        );
        let mac_dir = Path::new("/home/me/Library/Application Support/org.R-project.R/R/rok/r");
        let mac = r(
            "/home/me/Library/Application Support/org.R-project.R/R/rok/r/R-4.5.3",
            "/home/me/Library/Application Support/org.R-project.R/R/rok/r/R-4.5.3/bin/R",
            RKind::Managed,
        );
        let path = "~/Library/Application Support/org.R-project.R/R/rok/r/R-4.5.3/bin/R";
        assert_eq!(
            positron_settings(Os::MacOs, &mac, mac_dir, home).unwrap(),
            vec![
                ("positron.r.customBinaries", json!([path])),
                (DEFAULT_R, json!(path))
            ]
        );
        // Another R, or rok's R moved elsewhere: no settings.
        let system = r("/opt/R/4.5.3/lib/R", "/opt/R/4.5.3/bin/R", RKind::System);
        assert_eq!(positron_settings(Os::Linux, &system, rdir, home), None);
        let moved = r("/data/r/4.5.3/lib/R", "/data/r/4.5.3/bin/R", RKind::Managed);
        assert_eq!(positron_settings(Os::Linux, &moved, rdir, home), None);
    }

    #[cfg(windows)]
    #[test]
    fn names_rok_s_r_from_home_on_windows() {
        let home = Path::new(r"C:\Users\me");
        let rdir = Path::new(r"C:\Users\me\AppData\Local\Programs\R");
        let win = r(
            r"C:\Users\me\AppData\Local\Programs\R\R-4.5.3",
            r"C:\Users\me\AppData\Local\Programs\R\R-4.5.3\bin\R.exe",
            RKind::Managed,
        );
        assert_eq!(
            positron_settings(Os::Windows, &win, rdir, home).unwrap(),
            vec![(
                DEFAULT_R,
                json!("~/AppData/Local/Programs/R/R-4.5.3/bin/x64/R.exe")
            )]
        );
    }

    #[test]
    fn detects_positron() {
        assert!(in_positron(|k| (k == "POSITRON").then(|| "1".to_string())));
        assert!(in_positron(
            |k| (k == "TERM_PROGRAM").then(|| "Positron".to_string())
        ));
        assert!(!in_positron(
            |k| (k == "TERM_PROGRAM").then(|| "vscode".to_string())
        ));
        assert!(!in_positron(|_| None));
    }
}
