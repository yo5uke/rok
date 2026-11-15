# Installing the rok binary and placing this package where activated projects can see it
# (requirements chapter 4).

# Where releases of the binary are published (GitHub Releases); `options(rok.release_url = )`
# overrides it.
default_release_url <- "https://github.com/yo5uke/rok/releases/download"

# The Rust target name of this machine, as release files are named. Linux binaries are static
# (musl), so they run on any distribution.
rust_target <- function(sysname = Sys.info()[["sysname"]], machine = R.version$arch) {
  arch <- if (machine %in% c("aarch64", "arm64")) "aarch64" else "x86_64"
  switch(sysname,
    Linux = paste0(arch, "-unknown-linux-musl"),
    Darwin = paste0(arch, "-apple-darwin"),
    Windows = "x86_64-pc-windows-msvc",
    stop("rok has no binary for ", sysname, ".", call. = FALSE)
  )
}

# The release file of the binary for this machine (a .tar.gz on every OS).
release_file <- function(version, target = rust_target()) {
  base <- getOption("rok.release_url", default_release_url)
  sprintf("%s/v%s/rok-%s.tar.gz", base, version, target)
}

# Where this package is kept for activated projects: one library per R minor version.
package_library <- function() {
  file.path(data_dir(), "library", paste0("R-", current_r_minor()))
}

# Asks before rok writes outside the session's temporary directory. Without anyone to ask,
# only `options(rok.yes = TRUE)` agrees; `hint` says what else to do.
consent <- function(question, hint = "Run `rok::setup()` in an interactive session") {
  if (isTRUE(getOption("rok.yes"))) return(TRUE)
  if (!interactive()) {
    stop(question, "\n", "\u2139 ", hint, ", or set `options(rok.yes = TRUE)`.", call. = FALSE)
  }
  isTRUE(ask_yes_no(question, default = TRUE))
}

# What setup() does, as part of a question that starts with "rok will".
setup_plan <- function(binary = NULL) {
  sprintf(
    "put its engine (%s) in %s and a copy of the rok package in %s",
    if (is.null(binary)) "downloaded from GitHub" else "from the file you gave",
    dirname(binary_path()), package_library()
  )
}

#' Install the rok binary
#'
#' Downloads the rok binary that matches this package (or copies `binary`), and keeps a copy
#' of this package where activated projects can load it. Everything goes under
#' `tools::R_user_dir("rok", "data")`, after asking once. [init()] calls this when needed;
#' run it yourself to repair an installation or to install without the network.
#'
#' A downloaded binary is checked against the SHA-256 published with it, which needs R 4.5.0
#' or later (`tools::sha256sum()`); with an older R, only HTTPS protects the download, and
#' `setup()` says so.
#'
#' @param binary The path of a rok binary to install instead of downloading one.
#' @returns A list with the paths of the binary and of the package library, invisibly.
#' @export
#' @examplesIf interactive()
#' setup()
setup <- function(binary = NULL) {
  if (!consent(paste0("rok will ", setup_plan(binary), ". Continue?"))) {
    stop("Cancelled. Nothing was changed.", call. = FALSE)
  }
  install_engine(binary)
}

# setup() once agreed: installs the binary and places this package.
install_engine <- function(binary = NULL) {
  version <- as.character(utils::packageVersion("rok"))
  dest <- binary_path()
  lib <- package_library()
  source <- if (is.null(binary)) release_file(version) else path.expand(binary)

  if (is.null(binary) && is_dev_version(version)) {
    stop("rok ", version, " is a development version, so no release has its binary.\n",
      "\u2139 Build rok from the same source and run `rok::setup(binary = <path>)`.",
      call. = FALSE
    )
  }
  tmp <- tempfile("rok-setup-")
  dir.create(tmp)
  on.exit(unlink(tmp, recursive = TRUE), add = TRUE)
  new <- if (is.null(binary)) download_binary(source, tmp) else source
  if (!file.exists(new)) stop("`", new, "` does not exist.", call. = FALSE)
  found <- binary_version(new)
  if (is.na(found)) {
    stop("`", source, "` is not a rok binary that runs on this machine.", call. = FALSE)
  }
  if (!same_version(found, version)) {
    stop("`", source, "` is rok ", found, ", but this package needs rok ", version, ".",
      call. = FALSE
    )
  }

  dir.create(dirname(dest), recursive = TRUE, showWarnings = FALSE)
  # Copy beside the old binary, then swap it in.
  staged <- paste0(dest, ".new")
  if (!file.copy(new, staged, overwrite = TRUE)) {
    stop("Could not write `", staged, "`.", call. = FALSE)
  }
  Sys.chmod(staged, "755")
  if (!file.rename(staged, dest)) stop("Could not write `", dest, "`.", call. = FALSE)
  the$binary <- NULL

  place_package(lib)
  message("\u2714 Installed rok ", version, " in ", dirname(dest), ".")
  invisible(list(binary = dest, library = lib))
}

# A development version (0.2.0.9000) has a fourth component; releases have three.
is_dev_version <- function(version) {
  length(unclass(package_version(version))[[1L]]) > 3L
}

# Downloads `url` to `file`; FALSE if it cannot (download.file() signals most failures as
# errors and warnings, not by its value).
fetch <- function(url, file) {
  ok <- tryCatch(
    suppressWarnings(utils::download.file(url, file, mode = "wb", quiet = TRUE)) == 0L,
    error = function(e) FALSE
  )
  isTRUE(ok) && file.exists(file)
}

# tools::sha256sum(), which exists from R 4.5.0; NULL before.
sha256_fun <- function() {
  get0("sha256sum", envir = asNamespace("tools"), mode = "function", inherits = FALSE)
}

# Downloads a release file and checks it against the SHA-256 published beside it
# (`<url>.sha256`). Without one, nothing is installed: an unchecked program would run. R older
# than 4.5.0 cannot compute SHA-256, so there only HTTPS protects the download.
download_binary <- function(url, dir, sha256 = sha256_fun()) {
  file <- file.path(dir, basename(url))
  if (!fetch(url, file)) stop("Could not download ", url, ".", call. = FALSE)
  if (is.null(sha256)) {
    message(
      "! R ", getRversion(), " cannot compute SHA-256 (R 4.5.0 or later can), so the download ",
      "was not checked against its checksum; only HTTPS protected it."
    )
  } else {
    sum_file <- file.path(dir, "checksum")
    expected <- if (fetch(paste0(url, ".sha256"), sum_file)) {
      tolower(strsplit(trimws(readLines(sum_file, warn = FALSE)[1L]), "[[:space:]]+")[[1L]][1L])
    }
    if (length(expected) != 1L || is.na(expected) || !grepl("^[0-9a-f]{64}$", expected)) {
      stop("Could not get the SHA-256 of ", url, ", so the download cannot be checked; ",
        "nothing was installed.\n",
        "\u2139 Try again later, or download the binary yourself and run ",
        "`rok::setup(binary = <path>)`.",
        call. = FALSE
      )
    }
    if (!identical(unname(sha256(file)), expected)) {
      stop("The download of ", url, " is damaged (its SHA-256 does not match); ",
        "nothing was installed.",
        call. = FALSE
      )
    }
  }
  utils::untar(file, exdir = dir)
  found <- list.files(dir, pattern = paste0("^", exe("rok"), "$"), recursive = TRUE, full.names = TRUE)
  if (!length(found)) stop(url, " does not contain the rok binary.", call. = FALSE)
  found[[1L]]
}

# Copies the installed rok package into `lib`, unless it is loaded from there already.
place_package <- function(lib) {
  installed <- normalizePath(find.package("rok"))
  target <- file.path(lib, "rok")
  if (identical(installed, normalizePath(target, mustWork = FALSE))) return(invisible())
  dir.create(lib, recursive = TRUE, showWarnings = FALSE)
  staged <- file.path(lib, ".rok-new")
  unlink(staged, recursive = TRUE)
  dir.create(staged)
  on.exit(unlink(staged, recursive = TRUE), add = TRUE)
  if (!file.copy(installed, staged, recursive = TRUE)) {
    stop("Could not copy the rok package to `", lib, "`.", call. = FALSE)
  }
  unlink(target, recursive = TRUE)
  if (!file.rename(file.path(staged, basename(installed)), target)) {
    stop("Could not write `", target, "`.", call. = FALSE)
  }
  invisible()
}
