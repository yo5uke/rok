//! A rok project on disk.
//!
//! ```text
//! <root>/rok.toml                 manifest (written by users and rok)
//! <root>/rok.lock                 lockfile
//! <root>/.Rprofile                sources .rok/activate.R
//! <root>/.rok/activate.R          startup hook (base R only)
//! <root>/.rok/.gitignore          keeps machine-specific files out of Git
//! <root>/.rok/library/R-<minor>/<platform>/   project library (links into the cache)
//! <root>/.rok/undo/               the manifest and lockfile before the last change
//! ```

use std::path::{Path, PathBuf};

use crate::fsutil::write_atomic;
use crate::lockfile::{self, Lockfile, LockfileError};
use crate::manifest::{self, Manifest, ManifestDocument, ManifestError};
use crate::platform::Platform;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Lockfile(#[from] LockfileError),
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The line `.Rprofile` uses to run the startup hook.
pub const RPROFILE_LINE: &str = "source(\".rok/activate.R\")";

/// The startup hook. It must run with base R only, also where rok is not installed.
pub const ACTIVATE_R: &str = r#"# rok: activates this project's library. Created by rok; do not edit.
local({
  minor <- paste(R.version$major, sub("[.].*$", "", R.version$minor), sep = ".")
  arch <- R.version$arch
  tag <- if (.Platform$OS.type == "windows") {
    paste0("windows-", arch)
  } else if (Sys.info()[["sysname"]] == "Darwin") {
    paste0("macos-", arch)
  } else {
    # The same name as rok computes: ID and VERSION_ID from /etc/os-release.
    rel <- if (file.exists("/etc/os-release")) readLines("/etc/os-release", warn = FALSE) else character()
    field <- function(key) {
      line <- grep(paste0("^", key, "="), rel, value = TRUE)
      if (length(line)) gsub("[\"']", "", sub("^[^=]*=", "", line[[1L]])) else ""
    }
    id <- tolower(field("ID"))
    ver <- field("VERSION_ID")
    if (nzchar(id) && nzchar(ver)) paste(id, ver, arch, sep = "-")
    else if (nzchar(id)) paste(id, arch, sep = "-")
    else paste0("linux-", arch)
  }
  lib <- file.path(getwd(), ".rok", "library", paste0("R-", minor), tag)
  dir.create(lib, recursive = TRUE, showWarnings = FALSE)
  # Use only the project library and R's own library: no user or site libraries.
  assign(".lib.loc", unique(c(normalizePath(lib), .Library)), envir = environment(.libPaths))
})
"#;

/// Files that stay on each machine.
const ROK_GITIGNORE: &str = "library/\nundo/\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
}

impl Project {
    pub fn new(root: &Path) -> Project {
        Project {
            root: root.to_path_buf(),
        }
    }

    /// The project containing `dir`: the nearest directory, `dir` or above, with a `rok.toml`.
    pub fn find(dir: &Path) -> Option<Project> {
        dir.ancestors()
            .find(|d| d.join(manifest::FILE_NAME).is_file())
            .map(Project::new)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join(manifest::FILE_NAME)
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(lockfile::FILE_NAME)
    }

    pub fn rok_dir(&self) -> PathBuf {
        self.root.join(".rok")
    }

    /// The library for an R minor version (`4.6`) on a platform.
    pub fn library(&self, r_minor: &str, platform: &Platform) -> PathBuf {
        self.rok_dir()
            .join("library")
            .join(format!("R-{r_minor}"))
            .join(platform.library_tag())
    }

    pub fn undo_dir(&self) -> PathBuf {
        self.rok_dir().join("undo")
    }

    pub fn read_manifest(&self) -> Result<(ManifestDocument, Manifest), ProjectError> {
        let path = self.manifest_path();
        let text = std::fs::read_to_string(&path).map_err(io_err(&path))?;
        Ok(ManifestDocument::parse(&text)?)
    }

    /// The lockfile, or `None` if the project has none yet.
    pub fn read_lock(&self) -> Result<Option<Lockfile>, ProjectError> {
        let path = self.lock_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(Lockfile::parse(&text)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(&path)(e)),
        }
    }

    /// Writes the manifest and lockfile. With `backup`, the current files are first copied to
    /// `.rok/undo/` (one generation) so the change can be undone.
    pub fn save(&self, manifest: &str, lock: &Lockfile, backup: bool) -> Result<(), ProjectError> {
        if backup {
            let undo = self.undo_dir();
            for (from, name) in [
                (self.manifest_path(), manifest::FILE_NAME),
                (self.lock_path(), lockfile::FILE_NAME),
            ] {
                let to = undo.join(name);
                match std::fs::read(&from) {
                    Ok(bytes) => write_atomic(&to, &bytes).map_err(io_err(&to))?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        let _ = std::fs::remove_file(&to);
                    }
                    Err(e) => return Err(io_err(&from)(e)),
                }
            }
        }
        let (m, l) = (self.manifest_path(), self.lock_path());
        write_atomic(&m, manifest.as_bytes()).map_err(io_err(&m))?;
        write_atomic(&l, lock.to_toml_string().as_bytes()).map_err(io_err(&l))
    }

    /// Writes the lockfile only.
    pub fn save_lock(&self, lock: &Lockfile) -> Result<(), ProjectError> {
        let l = self.lock_path();
        write_atomic(&l, lock.to_toml_string().as_bytes()).map_err(io_err(&l))
    }

    /// Creates `.rok/activate.R` and `.rok/.gitignore`, and makes `.Rprofile` run the hook.
    /// Returns whether an existing `.Rprofile` was changed.
    pub fn install_startup_hook(&self) -> Result<bool, ProjectError> {
        let dir = self.rok_dir();
        for (name, content) in [("activate.R", ACTIVATE_R), (".gitignore", ROK_GITIGNORE)] {
            let p = dir.join(name);
            write_atomic(&p, content.as_bytes()).map_err(io_err(&p))?;
        }
        let rprofile = self.root.join(".Rprofile");
        match std::fs::read_to_string(&rprofile) {
            Ok(text) if text.lines().any(|l| l.trim() == RPROFILE_LINE) => Ok(false),
            Ok(text) => {
                // Run the hook first, before anything else in the user's .Rprofile.
                let new = format!("{RPROFILE_LINE}\n{text}");
                write_atomic(&rprofile, new.as_bytes()).map_err(io_err(&rprofile))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                write_atomic(&rprofile, format!("{RPROFILE_LINE}\n").as_bytes())
                    .map_err(io_err(&rprofile))?;
                Ok(false)
            }
            Err(e) => Err(io_err(&rprofile)(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_nearest_project() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("p");
        std::fs::create_dir_all(root.join("analysis/sub")).unwrap();
        std::fs::write(root.join("rok.toml"), "").unwrap();
        assert_eq!(
            Project::find(&root.join("analysis/sub")),
            Some(Project::new(&root))
        );
        assert_eq!(Project::find(t.path()), None);
    }

    #[test]
    fn installs_the_startup_hook_once() {
        let t = tempfile::tempdir().unwrap();
        let p = Project::new(t.path());
        std::fs::write(t.path().join(".Rprofile"), "options(digits = 4)\n").unwrap();
        assert!(p.install_startup_hook().unwrap());
        assert!(!p.install_startup_hook().unwrap());
        let text = std::fs::read_to_string(t.path().join(".Rprofile")).unwrap();
        assert_eq!(text, "source(\".rok/activate.R\")\noptions(digits = 4)\n");
        assert!(t.path().join(".rok/activate.R").is_file());
        assert_eq!(
            std::fs::read_to_string(t.path().join(".rok/.gitignore")).unwrap(),
            "library/\nundo/\n"
        );
    }

    #[test]
    fn saves_with_one_generation_of_backup() {
        let t = tempfile::tempdir().unwrap();
        let p = Project::new(t.path());
        let lock = Lockfile::parse(
            "version = 1\ngenerated-by = \"rok 0.1.0\"\n[r]\nversion = \"4.6.1\"\n[snapshot]\ndate = \"2026-10-01\"\nrepository = \"x\"\n",
        )
        .unwrap();
        p.save("first", &lock, true).unwrap();
        assert!(!p.undo_dir().join("rok.toml").exists());
        p.save("second", &lock, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.undo_dir().join("rok.toml")).unwrap(),
            "first"
        );
        assert_eq!(
            std::fs::read_to_string(p.manifest_path()).unwrap(),
            "second"
        );
        assert!(p.read_lock().unwrap().is_some());
    }
}
