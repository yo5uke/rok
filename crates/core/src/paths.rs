//! Where rok keeps its files outside projects.
//!
//! The locations follow R's `tools::R_user_dir("rok", which)`, so the engine and the R package
//! agree without talking to each other:
//!
//! | | data | cache |
//! |---|---|---|
//! | override | `ROK_DATA_DIR` | `ROK_CACHE_DIR` |
//! | R | `R_USER_DATA_DIR/R/rok` | `R_USER_CACHE_DIR/R/rok` |
//! | XDG | `XDG_DATA_HOME/R/rok` | `XDG_CACHE_HOME/R/rok` |
//! | Linux | `~/.local/share/R/rok` | `~/.cache/R/rok` |
//! | Windows | `%APPDATA%/R/data/R/rok` | `%LOCALAPPDATA%/R/cache/R/rok` |
//! | macOS | `~/Library/Application Support/org.R-project.R/R/rok` | `~/Library/Caches/org.R-project.R/R/rok` |
//!
//! `ROK_DATA_DIR` and `ROK_CACHE_DIR` name rok's directories directly (no `R/rok` is added);
//! they exist to isolate tests and benchmarks. Empty variables count as unset, as in R.
//!
//! R itself goes to `<data>/r/<version>`, except on Windows, where it goes to
//! `%LOCALAPPDATA%/Programs/R/R-<version>` beside R installed by CRAN's installer, so that IDEs
//! find it (requirements chapter 6). With `ROK_DATA_DIR` set, it stays under `<data>/r`.

use std::path::PathBuf;

use crate::platform::Os;

/// rok's per-user directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDirs {
    /// Persistent data: R installations, the R package.
    pub data: PathBuf,
    /// Re-creatable data: package indexes, downloaded and extracted packages.
    pub cache: PathBuf,
    /// Where rok installs R.
    pub r: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathsError {
    #[error("cannot find the home directory; set ROK_DATA_DIR and ROK_CACHE_DIR")]
    NoHome,
    #[error("the environment variable {0} is not set; set ROK_DATA_DIR and ROK_CACHE_DIR")]
    MissingVar(&'static str),
}

impl UserDirs {
    /// Resolves the directories from the current environment.
    pub fn from_env() -> Result<UserDirs, PathsError> {
        UserDirs::resolve(
            Os::current(),
            |k| std::env::var(k).ok(),
            std::env::home_dir(),
        )
    }

    /// Resolves the directories for `os`, reading variables with `var`.
    pub fn resolve(
        os: Os,
        var: impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
    ) -> Result<UserDirs, PathsError> {
        let var = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = || home.clone().ok_or(PathsError::NoHome);
        let base =
            |r_var: &str, xdg_var: &str, windows: (&'static str, &str), mac: &str, linux: &str| {
                if let Some(p) = var(r_var).or_else(|| var(xdg_var)) {
                    return Ok(p);
                }
                match os {
                    Os::Windows => var(windows.0)
                        .map(|p| p.join("R").join(windows.1))
                        .ok_or(PathsError::MissingVar(windows.0)),
                    Os::MacOs => Ok(home()?.join(mac)),
                    Os::Linux => Ok(home()?.join(linux)),
                }
            };
        let overridden = var("ROK_DATA_DIR");
        let data = match overridden.clone() {
            Some(p) => p,
            None => base(
                "R_USER_DATA_DIR",
                "XDG_DATA_HOME",
                ("APPDATA", "data"),
                "Library/Application Support/org.R-project.R",
                ".local/share",
            )?
            .join("R")
            .join("rok"),
        };
        let cache = match var("ROK_CACHE_DIR") {
            Some(p) => p,
            None => base(
                "R_USER_CACHE_DIR",
                "XDG_CACHE_HOME",
                ("LOCALAPPDATA", "cache"),
                "Library/Caches/org.R-project.R",
                ".cache",
            )?
            .join("R")
            .join("rok"),
        };
        let r = match (os, overridden) {
            (Os::Windows, None) => var("LOCALAPPDATA")
                .ok_or(PathsError::MissingVar("LOCALAPPDATA"))?
                .join("Programs")
                .join("R"),
            _ => data.join("r"),
        };
        Ok(UserDirs { data, cache, r })
    }

    /// Where rok installs R when no environment variable moves rok's directories: the same
    /// place, relative to the home directory, for everyone (Windows needs `LOCALAPPDATA`).
    pub fn default_r_installs(
        os: Os,
        var: impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
    ) -> Option<PathBuf> {
        let only_os = |k: &str| match k {
            "APPDATA" | "LOCALAPPDATA" => var(k),
            _ => None,
        };
        UserDirs::resolve(os, only_os, home).ok().map(|d| d.r)
    }

    /// Directories under one root, for tests: `<root>/data`, `<root>/cache`, `<root>/data/r`.
    pub fn under(root: &std::path::Path) -> UserDirs {
        UserDirs {
            data: root.join("data"),
            cache: root.join("cache"),
            r: root.join("data").join("r"),
        }
    }

    /// Where rok installs R ([`UserDirs::r`]).
    pub fn r_installs(&self) -> PathBuf {
        self.r.clone()
    }

    /// Cached data from P3M.
    pub fn p3m_cache(&self) -> PathBuf {
        self.cache.join("p3m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resolve(os: Os, vars: &[(&str, &str)]) -> Result<UserDirs, PathsError> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        UserDirs::resolve(os, |k| vars.get(k).cloned(), Some(PathBuf::from("/home/u")))
    }

    #[test]
    fn follows_r_user_dir_defaults() {
        let linux = resolve(Os::Linux, &[]).unwrap();
        assert_eq!(linux.data, PathBuf::from("/home/u/.local/share/R/rok"));
        assert_eq!(linux.cache, PathBuf::from("/home/u/.cache/R/rok"));
        assert_eq!(
            linux.r_installs(),
            PathBuf::from("/home/u/.local/share/R/rok/r")
        );
        let mac = resolve(Os::MacOs, &[]).unwrap();
        assert_eq!(
            mac.data,
            PathBuf::from("/home/u/Library/Application Support/org.R-project.R/R/rok")
        );
        assert_eq!(
            mac.cache,
            PathBuf::from("/home/u/Library/Caches/org.R-project.R/R/rok")
        );
        let win = resolve(
            Os::Windows,
            &[
                ("APPDATA", "C:/Users/u/AppData/Roaming"),
                ("LOCALAPPDATA", "C:/Users/u/AppData/Local"),
            ],
        )
        .unwrap();
        assert_eq!(
            win.data,
            PathBuf::from("C:/Users/u/AppData/Roaming/R/data/R/rok")
        );
        assert_eq!(
            win.cache,
            PathBuf::from("C:/Users/u/AppData/Local/R/cache/R/rok")
        );
        assert_eq!(
            win.r_installs(),
            PathBuf::from("C:/Users/u/AppData/Local/Programs/R")
        );
        // Tests and benchmarks keep R under their own data directory.
        let isolated = resolve(
            Os::Windows,
            &[
                ("ROK_DATA_DIR", "D:/t/data"),
                ("ROK_CACHE_DIR", "D:/t/cache"),
            ],
        )
        .unwrap();
        assert_eq!(isolated.r_installs(), PathBuf::from("D:/t/data/r"));
    }

    #[test]
    fn respects_variables_in_order() {
        let d = resolve(
            Os::Linux,
            &[("XDG_DATA_HOME", "/x/data"), ("XDG_CACHE_HOME", "/x/cache")],
        )
        .unwrap();
        assert_eq!(d.data, PathBuf::from("/x/data/R/rok"));
        assert_eq!(d.cache, PathBuf::from("/x/cache/R/rok"));
        let d = resolve(
            Os::Linux,
            &[("XDG_DATA_HOME", "/x/data"), ("R_USER_DATA_DIR", "/r/data")],
        )
        .unwrap();
        assert_eq!(d.data, PathBuf::from("/r/data/R/rok"));
        let d = resolve(
            Os::Linux,
            &[
                ("R_USER_CACHE_DIR", "/r/cache"),
                ("ROK_CACHE_DIR", "/rok/cache"),
                ("ROK_DATA_DIR", ""),
            ],
        )
        .unwrap();
        assert_eq!(d.cache, PathBuf::from("/rok/cache"));
        assert_eq!(d.data, PathBuf::from("/home/u/.local/share/R/rok"));
    }

    #[test]
    fn reports_missing_locations() {
        assert_eq!(
            resolve(Os::Windows, &[("LOCALAPPDATA", "C:/l")]),
            Err(PathsError::MissingVar("APPDATA"))
        );
        let none = UserDirs::resolve(Os::Linux, |_| None, None);
        assert_eq!(none, Err(PathsError::NoHome));
        let overridden = UserDirs::resolve(
            Os::Linux,
            |k| (k.starts_with("ROK_")).then(|| "/o".to_string()),
            None,
        );
        assert_eq!(overridden.unwrap().data, PathBuf::from("/o"));
    }
}
