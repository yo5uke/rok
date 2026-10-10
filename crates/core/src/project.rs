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

/// The file in `.rok/undo/` that marks a migration from renv as the change to undo.
const IMPORT_MARK: &str = "import-from-renv";

/// The file in `.rok/undo/` that keeps the IDE settings from before the last change, when the
/// change rewrote them: `present` or `absent`, a newline, then the text.
const IDE_SETTINGS_BACKUP: &str = "ide-settings";

/// The line `.Rprofile` uses to run the startup hook.
pub const RPROFILE_LINE: &str = "source(\".rok/activate.R\")";

/// The startup hook (requirements chapter 8). It must run with base R only, also where rok is
/// not installed. It asks the rok binary (`rok activate`) whether the library is in sync and
/// lets it sync, asks the person when the binary needs an answer, and then sets the library
/// paths: the project library, rok's own package, and R's library (no user or site library).
pub const ACTIVATE_R: &str = r#"# rok: activates this project's library and keeps it in sync. Created by rok; do not edit.
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
  # tools::R_user_dir("rok", "data"), written out: loading tools would slow every start.
  data <- Sys.getenv("ROK_DATA_DIR")
  if (!nzchar(data)) {
    base <- Sys.getenv("R_USER_DATA_DIR")
    if (!nzchar(base)) base <- Sys.getenv("XDG_DATA_HOME")
    if (!nzchar(base)) {
      base <- if (.Platform$OS.type == "windows") {
        file.path(Sys.getenv("APPDATA"), "R", "data")
      } else if (Sys.info()[["sysname"]] == "Darwin") {
        file.path(normalizePath("~"), "Library", "Application Support", "org.R-project.R")
      } else {
        file.path(normalizePath("~"), ".local", "share")
      }
    }
    data <- file.path(base, "R", "rok")
  }
  exe <- if (.Platform$OS.type == "windows") "rok.exe" else "rok"
  find_binary <- function() {
    for (bin in c(Sys.getenv("ROK_BINARY"), file.path(data, "bin", exe))) {
      if (nzchar(bin) && file.exists(bin)) return(bin)
    }
    # Searching PATH runs `which`, which costs more than everything else here: last resort.
    bin <- Sys.which("rok")
    if (nzchar(bin)) bin else NA_character_
  }
  bin <- find_binary()
  hint <- character()
  if (is.na(bin) && interactive()) {
    # A collaborator without rok: offer to install it (one question), then sync.
    if (nzchar(system.file(package = "rok"))) {
      # Prompts given: the askYesNo option can be a Windows dialog that opens behind Positron.
      if (isTRUE(utils::askYesNo("This project uses rok, whose engine is not installed. Install it now?",
                                 prompts = c("Yes", "No", "Cancel")))) {
        op <- options(rok.yes = TRUE)
        ok <- tryCatch({ rok::setup(); TRUE }, error = function(e) { message(conditionMessage(e)); FALSE })
        options(op)
        if (ok) bin <- find_binary()
      }
    } else {
      message("! This project uses rok to manage its packages, but rok is not installed.\n",
              "\u2139 Install it with `install.packages(\"rok\")`, then restart R.")
    }
  }
  if (!is.na(bin)) {
    args <- c("activate", "--r-home", R.home(), if (interactive()) "--interactive")
    repeat {
      # Capturing a program's output (stdout = TRUE) takes R about 0.2 s on Windows; sending it
      # to a file does not, so a file is used there (V13).
      file <- if (.Platform$OS.type == "windows") tempfile("rok-") else TRUE
      out <- tryCatch(
        suppressWarnings(system2(bin, shQuote(args), stdout = file, stderr = "")),
        error = function(e) structure(character(), status = 1L)
      )
      if (is.character(file)) {
        status <- out
        out <- if (file.exists(file)) readLines(file, warn = FALSE, encoding = "UTF-8") else character()
        unlink(file)
      } else {
        status <- attr(out, "status")
      }
      if (is.null(status) || !length(status)) status <- 0L
      value <- function(key) sub(paste0("^", key, "="), "", grep(paste0("^", key, "="), out, value = TRUE))
      if (status == 2L && length(value("choice"))) {
        choices <- value("option")
        pick <- utils::menu(sub("^[^\t]*\t", "", choices), title = value("question")[1L])
        # Cancelling picks the last option, which syncs nothing.
        if (pick == 0L) pick <- length(choices)
        args <- c(args, "--choice", sub("\t.*$", "", choices[[pick]]))
        next
      }
      # Strict mode in a non-interactive session: stop instead of running out of sync.
      if (status == 3L) quit(save = "no", status = 1L)
      if (length(value("library"))) lib <- value("library")[[1L]]
      if (length(value("hint_add")) && length(value("hint_sync"))) {
        hint <- c(add = value("hint_add")[[1L]], sync = value("hint_sync")[[1L]])
      }
      break
    }
  }
  dir.create(lib, recursive = TRUE, showWarnings = FALSE)
  # rok's own R package, so that rok::sync() and the others work in the project.
  own <- if (nzchar(data)) file.path(data, "library", paste0("R-", minor)) else character()
  own <- own[dir.exists(own)]
  assign(".lib.loc", unique(c(normalizePath(lib), own, .Library)), envir = environment(.libPaths))
  # A missing package that no code handles gets a line on what to do: errors from library(),
  # loadNamespace() and pkg::, and from rlang::check_installed(), which here raises an error
  # instead of offering install.packages() into the project library (where the next sync would
  # remove it). Those that tryCatch(), try() or requireNamespace() handle never reach the
  # handlers. R refuses to add them with handlers on the stack (an IDE running this file inside
  # tryCatch()): then none.
  if (length(hint)) options(rlib_restart_package_not_found = FALSE)
  on_stack <- function() {
    for (i in seq_len(sys.nframe())) {
      f <- sys.function(i)
      if (identical(f, tryCatch) || identical(f, withCallingHandlers)) return(TRUE)
    }
    FALSE
  }
  if (length(hint) && exists("globalCallingHandlers", baseenv()) && !on_stack()) {
    root <- getwd()
    # One line per package: sync if rok.lock has it, add if not. Packages rok does not manage,
    # and installed ones (older than asked for), get none.
    lines_for <- function(pkgs) {
      lock <- tryCatch(readLines(file.path(root, "rok.lock"), warn = FALSE), error = function(e) character())
      unlist(lapply(pkgs, function(pkg) {
        if (nzchar(system.file(package = pkg))) return(NULL)
        at <- match(paste0("name = \"", pkg, "\""), lock)
        kind <- "add"
        if (!is.na(at)) {
          rest <- lock[-seq_len(at)]
          entry <- rest[seq_len(match("", c(rest, "")) - 1L)]
          if (any(grepl("unmanaged = true", entry, fixed = TRUE))) return(NULL)
          kind <- "sync"
        }
        gsub("{package}", pkg, hint[[kind]], fixed = TRUE)
      }))
    }
    globalCallingHandlers(
      packageNotFoundError = function(cond) {
        pkg <- cond$package
        if (!is.character(pkg) || length(pkg) != 1L) return()
        lines <- lines_for(pkg)
        if (!length(lines)) return()
        cond$message <- paste(c(conditionMessage(cond), lines), collapse = "\n")
        stop(cond)
      },
      rlib_error_package_not_found = function(cond) {
        if (!is.character(cond$pkg)) return()
        lines <- lines_for(cond$pkg)
        if (!length(lines)) return()
        cond$footer <- c(cond$footer, lines)
        stop(cond)
      }
    )
  }
})
"#;

/// Files that stay on each machine.
const ROK_GITIGNORE: &str = "library/\nundo/\nscan.json\n";

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
            // This change is now the one undo reverts, not an earlier migration from renv or an
            // earlier change of the IDE settings.
            let _ = std::fs::remove_file(self.undo_dir().join(IMPORT_MARK));
            let _ = std::fs::remove_file(self.undo_dir().join(IDE_SETTINGS_BACKUP));
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
        write_atomic(&l, lock.to_toml_string().as_bytes()).map_err(io_err(&l))?;
        self.export_renv_if_asked(manifest, lock)
    }

    /// Writes renv.lock beside rok.lock when rok.toml asks for it (`[export] renv = true`).
    fn export_renv_if_asked(&self, manifest: &str, lock: &Lockfile) -> Result<(), ProjectError> {
        if Manifest::parse(manifest).is_ok_and(|m| m.export_renv) {
            let p = self.root.join(crate::renv::FILE_NAME);
            write_atomic(&p, crate::renv::export(lock).as_bytes()).map_err(io_err(&p))?;
        }
        Ok(())
    }

    /// The manifest text and lockfile saved before the last change, if any.
    pub fn read_undo(&self) -> Result<Option<(String, Option<Lockfile>)>, ProjectError> {
        let dir = self.undo_dir();
        let manifest = dir.join(manifest::FILE_NAME);
        let text = match std::fs::read_to_string(&manifest) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err(&manifest)(e)),
        };
        let lock = dir.join(lockfile::FILE_NAME);
        let lock = match std::fs::read_to_string(&lock) {
            Ok(t) => Some(Lockfile::parse(&t)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_err(&lock)(e)),
        };
        Ok(Some((text, lock)))
    }

    /// Writes the IDE settings file (`.vscode/settings.json`). With `backup`, the current file
    /// is kept in `.rok/undo/` first, so undoing the change that came with it restores it.
    pub fn write_ide_settings(&self, text: &str, backup: bool) -> Result<(), ProjectError> {
        let path = self.root.join(crate::ide::SETTINGS_FILE);
        if backup {
            let saved = match std::fs::read_to_string(&path) {
                Ok(old) => format!("present\n{old}"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent\n".to_string(),
                Err(e) => return Err(io_err(&path)(e)),
            };
            let to = self.undo_dir().join(IDE_SETTINGS_BACKUP);
            write_atomic(&to, saved.as_bytes()).map_err(io_err(&to))?;
        }
        write_atomic(&path, text.as_bytes()).map_err(io_err(&path))
    }

    /// Restores the IDE settings saved by [`Project::write_ide_settings`], if any. Returns
    /// whether there were any.
    pub fn undo_ide_settings(&self) -> Result<bool, ProjectError> {
        let saved_path = self.undo_dir().join(IDE_SETTINGS_BACKUP);
        let saved = match std::fs::read_to_string(&saved_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(io_err(&saved_path)(e)),
        };
        let path = self.root.join(crate::ide::SETTINGS_FILE);
        match saved.split_once('\n') {
            Some(("present", text)) => {
                write_atomic(&path, text.as_bytes()).map_err(io_err(&path))?
            }
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
        std::fs::remove_file(&saved_path).map_err(io_err(&saved_path))?;
        Ok(true)
    }

    /// Forgets the saved state (after it was restored).
    pub fn clear_undo(&self) -> Result<(), ProjectError> {
        for name in [manifest::FILE_NAME, lockfile::FILE_NAME] {
            let p = self.undo_dir().join(name);
            match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_err(&p)(e)),
                _ => {}
            }
        }
        Ok(())
    }

    /// Writes the lockfile only.
    pub fn save_lock(&self, lock: &Lockfile) -> Result<(), ProjectError> {
        let l = self.lock_path();
        write_atomic(&l, lock.to_toml_string().as_bytes()).map_err(io_err(&l))?;
        let manifest = std::fs::read_to_string(self.manifest_path()).unwrap_or_default();
        self.export_renv_if_asked(&manifest, lock)
    }

    /// After a migration from renv: makes `.Rprofile` run rok's hook instead of renv's (only
    /// that line changes; without one, rok's is added first), and remembers the old `.Rprofile`
    /// so that undo can revert the migration.
    pub fn take_over_from_renv(&self) -> Result<(), ProjectError> {
        self.refresh_activate()?;
        let rprofile = self.root.join(".Rprofile");
        let old = match std::fs::read_to_string(&rprofile) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io_err(&rprofile)(e)),
        };
        let renv_line = |l: &str| {
            let t = l.trim().replace('\'', "\"");
            t == "source(\"renv/activate.R\")"
        };
        let new = match &old {
            Some(t) if t.lines().any(renv_line) => {
                t.lines()
                    .map(|l| if renv_line(l) { RPROFILE_LINE } else { l })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            }
            Some(t) if t.lines().any(|l| l.trim() == RPROFILE_LINE) => t.clone(),
            Some(t) => format!("{RPROFILE_LINE}\n{t}"),
            None => format!("{RPROFILE_LINE}\n"),
        };
        write_atomic(&rprofile, new.as_bytes()).map_err(io_err(&rprofile))?;
        // What undo needs: whether .Rprofile existed, and its text.
        let mark = self.undo_dir().join(IMPORT_MARK);
        let saved = match &old {
            Some(t) => format!("present\n{t}"),
            None => "absent\n".to_string(),
        };
        write_atomic(&mark, saved.as_bytes()).map_err(io_err(&mark))
    }

    /// Whether the last change was a migration from renv.
    pub fn imported_from_renv(&self) -> bool {
        self.undo_dir().join(IMPORT_MARK).is_file()
    }

    /// Reverts a migration from renv: restores `.Rprofile` and removes rok.toml and rok.lock.
    /// The library under `.rok/` stays (it is only links into the cache).
    pub fn undo_import(&self) -> Result<(), ProjectError> {
        let mark = self.undo_dir().join(IMPORT_MARK);
        let saved = std::fs::read_to_string(&mark).map_err(io_err(&mark))?;
        let rprofile = self.root.join(".Rprofile");
        match saved.split_once('\n') {
            Some(("present", text)) => {
                write_atomic(&rprofile, text.as_bytes()).map_err(io_err(&rprofile))?
            }
            _ => {
                let _ = std::fs::remove_file(&rprofile);
            }
        }
        for p in [self.manifest_path(), self.lock_path(), mark] {
            match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(io_err(&p)(e)),
                _ => {}
            }
        }
        self.clear_undo()
    }

    /// Rewrites `.rok/activate.R` and `.rok/.gitignore` if they differ from this version of
    /// rok's (after rok was updated). Returns whether anything changed.
    pub fn refresh_activate(&self) -> Result<bool, ProjectError> {
        let mut changed = false;
        for (name, content) in [("activate.R", ACTIVATE_R), (".gitignore", ROK_GITIGNORE)] {
            let p = self.rok_dir().join(name);
            if std::fs::read_to_string(&p).is_ok_and(|t| t == content) {
                continue;
            }
            write_atomic(&p, content.as_bytes()).map_err(io_err(&p))?;
            changed = true;
        }
        Ok(changed)
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
            "library/\nundo/\nscan.json\n"
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
