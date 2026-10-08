# Installing the rok binary and placing this package where activated projects can see it
# (requirements chapter 4).

# Where releases of the binary are published. The final location is decided when releases
# start (roadmap 2-10); `options(rok.release_url = )` overrides it.
default_release_url <- "https://github.com/yo5uke/rok/releases/download"

# The Rust target name of this machine, as release files are named.
rust_target <- function(sysname = Sys.info()[["sysname"]], machine = R.version$arch) {
  arch <- if (machine %in% c("aarch64", "arm64")) "aarch64" else "x86_64"
  switch(sysname,
    Linux = paste0(arch, "-unknown-linux-gnu"),
    Darwin = paste0(arch, "-apple-darwin"),
    Windows = "x86_64-pc-windows-msvc",
    stop("rok has no binary for ", sysname, ".", call. = FALSE)
  )
}

# The release file of the binary for this machine.
release_file <- function(version, target = rust_target()) {
  ext <- if (grepl("windows", target, fixed = TRUE)) ".zip" else ".tar.gz"
  base <- getOption("rok.release_url", default_release_url)
  sprintf("%s/v%s/rok-%s%s", base, version, target, ext)
}

# Where this package is kept for activated projects: one library per R minor version.
package_library <- function() {
  file.path(data_dir(), "library", paste0("R-", current_r_minor()))
}

consent <- function(question) {
  if (isTRUE(getOption("rok.yes"))) return(TRUE)
  if (!interactive()) {
    stop(question, "\n",
      "i Run `rok::setup()` in an interactive session, or set `options(rok.yes = TRUE)`.",
      call. = FALSE
    )
  }
  isTRUE(utils::askYesNo(question, default = TRUE))
}

#' Install the rok binary
#'
#' Downloads the rok binary that matches this package (or copies `binary`), and keeps a copy
#' of this package where activated projects can load it. Everything goes under
#' `tools::R_user_dir("rok", "data")`, after asking once. [init()] calls this when needed;
#' run it yourself to repair an installation or to install without the network.
#'
#' @param binary The path of a rok binary to install instead of downloading one.
#' @return The paths of the binary and of the package library, invisibly.
#' @export
#' @examples
#' \dontrun{
#' setup()
#' setup(binary = "~/Downloads/rok")
#' }
setup <- function(binary = NULL) {
  version <- as.character(utils::packageVersion("rok"))
  dest <- binary_path()
  lib <- package_library()
  source <- if (is.null(binary)) release_file(version) else path.expand(binary)
  question <- sprintf(
    "rok will put its engine (%s) in %s and a copy of the rok package in %s. Continue?",
    if (is.null(binary)) "downloaded from GitHub" else "from the file you gave",
    dirname(dest), lib
  )
  if (!consent(question)) stop("Cancelled. Nothing was changed.", call. = FALSE)

  tmp <- tempfile("rok-setup-")
  dir.create(tmp)
  on.exit(unlink(tmp, recursive = TRUE), add = TRUE)
  new <- if (is.null(binary)) download_binary(source, tmp) else source
  if (!file.exists(new)) stop("`", new, "` does not exist.", call. = FALSE)
  found <- binary_version(new)
  if (!identical(found, version)) {
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
  message("\u2714 Installed rok ", version, " in ", dirname(dest))
  invisible(list(binary = dest, library = lib))
}

download_binary <- function(url, dir) {
  file <- file.path(dir, basename(url))
  status <- tryCatch(
    utils::download.file(url, file, mode = "wb", quiet = TRUE),
    error = function(e) stop("Could not download ", url, ": ", conditionMessage(e), call. = FALSE)
  )
  if (status != 0L) stop("Could not download ", url, ".", call. = FALSE)
  # Release files come with their SHA-256 (tools::sha256sum() exists from R 4.5.0).
  sum_file <- file.path(dir, "checksum")
  if (exists("sha256sum", envir = asNamespace("tools")) &&
      utils::download.file(paste0(url, ".sha256"), sum_file, quiet = TRUE) == 0L) {
    expected <- strsplit(readLines(sum_file, warn = FALSE)[[1L]], "[[:space:]]+")[[1L]][[1L]]
    actual <- unname(get("sha256sum", envir = asNamespace("tools"))(file))
    if (!identical(tolower(expected), actual)) {
      stop("The download of ", url, " is damaged (SHA-256 mismatch).", call. = FALSE)
    }
  }
  if (grepl("\\.zip$", file)) utils::unzip(file, exdir = dir) else utils::untar(file, exdir = dir)
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
  if (!file.copy(installed, staged, recursive = TRUE)) {
    stop("Could not copy the rok package to `", lib, "`.", call. = FALSE)
  }
  unlink(target, recursive = TRUE)
  file.rename(file.path(staged, basename(installed)), target)
  unlink(staged, recursive = TRUE)
  invisible()
}
