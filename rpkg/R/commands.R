# One function per command (requirements chapter 5). Each returns the command's result
# invisibly; the messages come from the binary. `project` is the CLI's `--project`: the
# project that contains that directory.

current_r_minor <- function() {
  paste(R.version$major, strsplit(R.version$minor, ".", fixed = TRUE)[[1L]][[1L]], sep = ".")
}

project_arg <- function(project) {
  c("--project", path.expand(project))
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
#' @returns The result, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' list.files(dir, all.files = TRUE, no.. = TRUE)
init <- function(path = ".", r = NULL, name = NULL) {
  # The first init installs the binary, after asking once.
  if (!rok_available()) setup()
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
#' @param project A directory of the project (default: the working directory).
#' @returns The result (the changes), invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' add("R6", project = dir)
#' add("jsonlite", version = ">= 1.8", project = dir)
#'
#' # GitHub packages, optionally at a tag, branch or commit
#' add("gaborcsardi/praise", project = dir)
add <- function(..., version = NULL, latest = FALSE, project = ".") {
  pkgs <- c(...)
  if (!length(pkgs)) stop("Give at least one package.", call. = FALSE)
  invisible(rok_call(c(
    "add", pkgs, opt("--version", version), if (latest) "--latest", project_arg(project)
  )))
}

#' Remove packages
#'
#' Removes packages from `rok.toml`, then updates `rok.lock` and the project library. The
#' packages stay in rok's cache.
#'
#' When rok is attached, this function masks [base::remove()]. Calls meant for it still reach
#' it: if an argument is an unquoted name (`remove(x)`), or `list`, `pos`, `envir` or
#' `inherits` is given, the call goes to the `remove()` that rok masks. To remove packages
#' whose names are in a variable, call `rok::remove(pkgs)`.
#'
#' @param ... Package names (or `"owner/repo"` for GitHub packages).
#' @param project A directory of the project (default: the working directory).
#' @returns The result (the changes), invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' add("R6", project = dir)
#' remove("R6", project = dir)
remove <- function(..., project = ".") {
  call <- sys.call()
  if (!namespaced(call) && meant_for_base_remove(substitute(...()))) {
    return(pass_through(call, parent.frame(), "remove", base::remove))
  }
  pkgs <- c(...)
  if (!length(pkgs)) stop("Give at least one package.", call. = FALSE)
  invisible(rok_call(c("remove", pkgs, project_arg(project))))
}

#' Make the library match the lockfile
#'
#' Updates `rok.lock` first if `rok.toml` changed.
#'
#' @param locked Fail instead of updating `rok.lock` when it is out of date.
#' @inheritParams add
#' @returns The result, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' sync(project = dir)
sync <- function(locked = FALSE, project = ".") {
  invisible(rok_call(c("sync", if (locked) "--locked", project_arg(project))))
}

#' Update packages
#'
#' Without packages, moves the project's snapshot date to the latest one (or `to`) and
#' resolves again. With packages, updates only those (and what they need) from the latest
#' snapshot, keeping the project's date.
#'
#' When rok is attached, this function masks [stats::update()]. Calls meant for it still
#' reach it: if the first argument is not a character vector (a fitted model or a formula,
#' as in `update(fit, . ~ . + x)`), the call goes to the `update()` that rok masks, as if rok
#' were not attached.
#'
#' @param ... Packages to update (default: all).
#' @param to The snapshot date to move to.
#' @param dry_run Show what would change without changing anything.
#' @inheritParams add
#' @returns The result (the changes), invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' add("R6", project = dir)
#' update(dry_run = TRUE, project = dir)
#' update("R6", project = dir)
#'
#' # Models are still updated by stats::update()
#' fit <- lm(dist ~ speed, data = cars)
#' update(fit, . ~ . + I(speed^2))
update <- function(..., to = NULL, dry_run = FALSE, project = ".") {
  if (...length() && !is.character(..1)) {
    return(pass_through(sys.call(), parent.frame(), "update", stats::update, first = ..1))
  }
  pkgs <- c(...)
  if (...length() && !length(pkgs)) {
    stop("No packages were given; call `update()` to update the whole project.", call. = FALSE)
  }
  invisible(rok_call(c(
    "update", pkgs, opt("--to", to), if (dry_run) "--dry-run", project_arg(project)
  )))
}

#' Undo the last change
#'
#' Restores `rok.toml` and `rok.lock` as they were before the last [add()], [remove()],
#' [update()] or [pin_r()], and the library with them.
#'
#' @inheritParams add
#' @returns The result, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' add("R6", project = dir)
#' undo(project = dir)
undo <- function(project = ".") {
  invisible(rok_call(c("undo", project_arg(project))))
}

#' Show the project's status
#'
#' Shows what is out of sync between `rok.toml`, `rok.lock`, the library and R, without
#' using the network, and returns the details invisibly.
#'
#' @param packages List every package.
#' @inheritParams add
#' @returns A list, invisibly: `ok` (whether there is nothing to fix), `problems` (a data
#'   frame with one row per problem) and `packages` (a data frame with one row per package),
#'   among others.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' s <- status(project = dir)
#' s$ok
status <- function(packages = FALSE, project = ".") {
  res <- rok_call(c("status", if (packages) "--packages", project_arg(project)))
  res$problems <- records_df(res$problems)
  res$packages <- records_df(res$packages)
  invisible(res)
}

#' Why a package is installed
#'
#' Shows the packages that need `pkg`, up to the declared ones.
#'
#' @param pkg A package name.
#' @inheritParams add
#' @returns The lines of the drawing, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' add("jsonlite", project = dir)
#' why("jsonlite", project = dir)
why <- function(pkg, project = ".") {
  res <- rok_call(c("why", pkg, project_arg(project)))
  cat(res$lines, sep = "\n")
  invisible(res$lines)
}

#' Show the dependency tree
#'
#' @param pkg Show only this package's dependencies (default: every declared package).
#' @param depth How many levels to show.
#' @inheritParams add
#' @returns The lines of the drawing, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' add("R6", project = dir)
#' tree(project = dir)
tree <- function(pkg = NULL, depth = NULL, project = ".") {
  res <- rok_call(c("tree", pkg, opt("--depth", depth), project_arg(project)))
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
#' @inheritParams add
#' @returns The exit status of the script, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' script <- file.path(dir, "hello.R")
#' writeLines('cat("Hello from", R.version.string, "\\n")', script)
#' run(script, project = dir)
run <- function(file, args = character(), r = NULL, project = ".") {
  status <- system2(
    rok_binary(),
    shQuote(c(project_arg(project), "run", opt("--r", r), path.expand(file), args))
  )
  invisible(status)
}

#' Change the project's R version
#'
#' Moves the project to another minor version of R and resolves its packages again. If some
#' packages have no binary for the new R, asks how to handle them. If the new R is not
#' installed, offers to install it.
#'
#' @param version A version such as `"4.6"`, `"4.6.1"` or `"latest"`.
#' @param strategy What to do with packages without binaries: `"move"`, `"build"`, `"date"`
#'   or `"today"` (asked if not given).
#' @inheritParams add
#' @returns The result, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' dir <- tempfile("my-analysis")
#' init(dir)
#' pin_r("latest", project = dir)
pin_r <- function(version, strategy = NULL, project = ".") {
  invisible(rok_call(c(
    "r", "pin", version, opt("--strategy", strategy), project_arg(project)
  )))
}

#' Install R
#'
#' Installs R without administrator rights. Without `version`, installs the version the
#' project in the working directory needs (or, outside a project, the latest release).
#'
#' @param version A version such as `"4.6"`, `"4.6.1"` or `"latest"`.
#' @param system Only show the commands that install R for all users (they need `sudo`).
#' @returns The result, invisibly.
#' @export
#' @examplesIf interactive() && rok_available()
#' # Show how to install the latest R for all users (nothing is installed)
#' install_r("latest", system = TRUE)
install_r <- function(version = NULL, system = FALSE) {
  invisible(rok_call(c("r", "install", version, if (system) "--system")))
}

# ---- functions that rok masks ----

# Whether the call names rok explicitly (`rok::remove()`), rather than finding it on the
# search path.
namespaced <- function(call) {
  f <- call[[1L]]
  is.call(f) && (identical(f[[1L]], as.name("::")) || identical(f[[1L]], as.name(":::")))
}

# Whether a call to remove() is meant for base::remove(): an unquoted name, or one of its
# own arguments. `exprs` are the unevaluated arguments.
meant_for_base_remove <- function(exprs) {
  if (!length(exprs)) return(FALSE)
  nms <- names(exprs)
  if (!is.null(nms) && any(nms %in% c("list", "pos", "envir", "inherits"))) return(TRUE)
  any(vapply(exprs, is.name, logical(1)))
}

# The function that `name` refers to when rok is not on the search path: the next one after
# package:rok (stats::update, or another package's, such as an S4 generic), or `fallback`.
masked <- function(name, fallback) {
  ours <- get(name, envir = asNamespace("rok"), inherits = FALSE)
  pos <- match("package:rok", search())
  from <- if (is.na(pos)) globalenv() else as.environment(pos + 1L)
  f <- get0(name, envir = from, mode = "function")
  if (is.null(f) || identical(f, ours)) fallback else f
}

# Evaluates `call` again in the caller's frame `env`, with the masked function instead of
# rok's, so it behaves exactly as if rok were not attached: the arguments keep their
# expressions (functions such as update() and remove() read them with match.call()) and the
# frames are the caller's. `first`, if given, is the already evaluated first argument: an
# argument written as a call (`update(fit(), ...)`) is replaced by its value, so it is not
# evaluated twice.
pass_through <- function(call, env, name, fallback, first = NULL) {
  f <- masked(name, fallback)
  call[[1L]] <- if (identical(f, fallback)) {
    call("::", as.name(environmentName(environment(fallback))), as.name(name))
  } else {
    f
  }
  if (!is.null(first) && length(call) >= 2L) {
    args <- as.list(call)[-1L]
    nms <- if (is.null(names(args))) rep("", length(args)) else names(args)
    # The first argument that went to `...` (rok's own arguments are matched exactly).
    i <- which(!nms %in% c("to", "dry_run", "project"))[1L]
    if (!is.na(i) && is.call(args[[i]])) call[[i + 1L]] <- first
  }
  eval(call, env)
}
