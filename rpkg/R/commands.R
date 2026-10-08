# One function per command (requirements chapter 5). Each returns the command's result
# invisibly; the messages come from the binary.

current_r_minor <- function() {
  paste(R.version$major, strsplit(R.version$minor, ".", fixed = TRUE)[[1L]][[1L]], sep = ".")
}

#' Create a project
#'
#' Creates `rok.toml`, `rok.lock` and the startup hook in `path`. The project uses the minor
#' version of the R that is running, and the latest snapshot date that Posit Package Manager
#' has published. If the rok binary is not installed yet, [setup()] installs it first.
#'
#' @param path The project directory (created if needed).
#' @param r The R version, such as `"4.6"` or `"latest"` (default: the running R).
#' @param name The project name (default: the directory name).
#' @return The result, invisibly.
#' @export
#' @examples
#' \dontrun{
#' init()
#' init("~/projects/my-analysis", r = "4.5")
#' }
init <- function(path = ".", r = NULL, name = NULL) {
  # The first init installs the binary, after asking once.
  if (inherits(tryCatch(rok_binary(), error = identity), "error")) setup()
  if (is.null(r)) r <- current_r_minor()
  invisible(rok_call(c("init", path.expand(path), opt("--r", r), opt("--name", name))))
}

#' Add packages
#'
#' Declares packages in `rok.toml`, then updates `rok.lock` and the project library. Accepts
#' the same forms as pak: `"fixest"` from CRAN, `"owner/repo"` and `"owner/repo@ref"` from
#' GitHub.
#'
#' @param ... Package names or GitHub repositories.
#' @param version A version constraint for one package, such as `"< 0.13"`.
#' @param latest Take the packages from the latest snapshot, keeping the project's snapshot.
#' @return The result (the changes), invisibly.
#' @export
#' @examples
#' \dontrun{
#' add("fixest", "sf")
#' add("fixest", version = "< 0.13")
#' add("yo5uke/coresynth@v0.3.0")
#' }
add <- function(..., version = NULL, latest = FALSE) {
  pkgs <- c(...)
  if (!length(pkgs)) stop("Give at least one package.", call. = FALSE)
  invisible(rok_call(c("add", pkgs, opt("--version", version), if (latest) "--latest")))
}

#' Remove packages
#'
#' Removes packages from `rok.toml`, then updates `rok.lock` and the project library. The
#' packages stay in rok's cache.
#'
#' @param ... Package names (or `"owner/repo"` for GitHub packages).
#' @return The result (the changes), invisibly.
#' @export
#' @examples
#' \dontrun{
#' remove("sf")
#' }
remove <- function(...) {
  pkgs <- c(...)
  if (!length(pkgs)) stop("Give at least one package.", call. = FALSE)
  invisible(rok_call(c("remove", pkgs)))
}

#' Make the library match the lockfile
#'
#' Updates `rok.lock` first if `rok.toml` changed.
#'
#' @param locked Fail instead of updating `rok.lock` when it is out of date.
#' @return The result, invisibly.
#' @export
#' @examples
#' \dontrun{
#' sync()
#' }
sync <- function(locked = FALSE) {
  invisible(rok_call(c("sync", if (locked) "--locked")))
}

#' Update packages
#'
#' Without packages, moves the project's snapshot date to the latest one (or `to`) and
#' resolves again. With packages, updates only those (and what they need) from the latest
#' snapshot, keeping the project's date.
#'
#' @param ... Packages to update (default: all).
#' @param to The snapshot date to move to.
#' @param dry_run Show what would change without changing anything.
#' @return The result (the changes), invisibly.
#' @export
#' @examples
#' \dontrun{
#' update()
#' update("sf")
#' update(dry_run = TRUE)
#' }
update <- function(..., to = NULL, dry_run = FALSE) {
  invisible(rok_call(c("update", c(...), opt("--to", to), if (dry_run) "--dry-run")))
}

#' Undo the last change
#'
#' Restores `rok.toml` and `rok.lock` as they were before the last [add()], [remove()],
#' [update()] or [pin_r()], and the library with them.
#'
#' @return The result, invisibly.
#' @export
#' @examples
#' \dontrun{
#' undo()
#' }
undo <- function() {
  invisible(rok_call("undo"))
}

#' Show the project's status
#'
#' Shows what is out of sync between `rok.toml`, `rok.lock`, the library and R, without
#' using the network, and returns the details invisibly.
#'
#' @param packages List every package.
#' @return A list with the project, the problems (a data frame) and the packages (a data
#'   frame), invisibly.
#' @export
#' @examples
#' \dontrun{
#' s <- status()
#' s$ok
#' }
status <- function(packages = FALSE) {
  res <- rok_call(c("status", if (packages) "--packages"))
  res$problems <- records_df(res$problems)
  res$packages <- records_df(res$packages)
  invisible(res)
}

#' Why a package is installed
#'
#' Shows the packages that need `pkg`, up to the declared ones.
#'
#' @param pkg A package name.
#' @return The lines of the drawing, invisibly.
#' @export
#' @examples
#' \dontrun{
#' why("data.table")
#' }
why <- function(pkg) {
  res <- rok_call(c("why", pkg))
  cat(res$lines, sep = "\n")
  invisible(res$lines)
}

#' Show the dependency tree
#'
#' @param pkg Show only this package's dependencies (default: every declared package).
#' @param depth How many levels to show.
#' @return The lines of the drawing, invisibly.
#' @export
#' @examples
#' \dontrun{
#' tree()
#' tree("fixest", depth = 1)
#' }
tree <- function(pkg = NULL, depth = NULL) {
  res <- rok_call(c("tree", pkg, opt("--depth", depth)))
  cat(res$lines, sep = "\n")
  invisible(res$lines)
}

#' Run a script with the project's R
#'
#' Syncs the project, then runs `file` with `Rscript` in a separate process, using the
#' project's R (or `r`) and library.
#'
#' @param file The script.
#' @param args Arguments passed to the script.
#' @param r The R version to use instead of the project's, such as `"4.5"`.
#' @return The exit status of the script, invisibly.
#' @export
#' @examples
#' \dontrun{
#' run("analysis.R")
#' run("analysis.R", r = "4.5")
#' }
run <- function(file, args = character(), r = NULL) {
  status <- system2(rok_binary(), shQuote(c("run", opt("--r", r), file, args)))
  invisible(status)
}

#' Change the project's R version
#'
#' Moves the project to another minor version of R and resolves its packages again. If some
#' packages have no binary for the new R, asks how to handle them.
#'
#' @param version A version such as `"4.6"`, `"4.6.1"` or `"latest"`.
#' @param strategy What to do with packages without binaries: `"move"`, `"build"`, `"date"`
#'   or `"today"` (asked if not given).
#' @return The result, invisibly.
#' @export
#' @examples
#' \dontrun{
#' pin_r("4.6")
#' }
pin_r <- function(version, strategy = NULL) {
  invisible(rok_call(c("r", "pin", version, opt("--strategy", strategy))))
}

#' Install R
#'
#' Installs R without administrator rights. Without `version`, installs the version the
#' project needs (or, outside a project, the latest release).
#'
#' @param version A version such as `"4.6"`, `"4.6.1"` or `"latest"`.
#' @param system Only show the commands that install R for all users (they need `sudo`).
#' @return The result, invisibly.
#' @export
#' @examples
#' \dontrun{
#' install_r("4.5")
#' }
install_r <- function(version = NULL, system = FALSE) {
  invisible(rok_call(c("r", "install", version, if (system) "--system")))
}
