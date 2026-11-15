# Release names, and (when ROK_TEST_BINARY points to a rok binary of the same version) setup()
# and a call through the binary. Everything is written under a temporary directory.
stopifnot(
  identical(rok:::rust_target("Linux", "x86_64"), "x86_64-unknown-linux-musl"),
  identical(rok:::rust_target("Linux", "aarch64"), "aarch64-unknown-linux-musl"),
  identical(rok:::rust_target("Darwin", "arm64"), "aarch64-apple-darwin"),
  identical(rok:::rust_target("Windows", "x86_64"), "x86_64-pc-windows-msvc"),
  identical(
    rok:::release_file("0.1.0", "x86_64-unknown-linux-musl"),
    "https://github.com/yo5uke/rok/releases/download/v0.1.0/rok-x86_64-unknown-linux-musl.tar.gz"
  ),
  identical(
    rok:::release_file("0.1.0", "x86_64-pc-windows-msvc"),
    "https://github.com/yo5uke/rok/releases/download/v0.1.0/rok-x86_64-pc-windows-msvc.tar.gz"
  ),
  identical(rok:::opt("--to", NULL), character()),
  # The binary writes a development version with `-` (Cargo's form).
  rok:::same_version("0.2.0-9000", "0.2.0.9000"),
  !rok:::same_version("0.2.0", "0.2.0.9000"),
  rok:::is_dev_version("0.2.0.9000"),
  !rok:::is_dev_version("0.2.1"),
  identical(rok:::opt("--depth", 2), c("--depth", "2"))
)

# Questions are asked in the console even when the askYesNo option is a dialog (as in Positron
# on Windows). Without a console to answer, the default is taken.
local({
  op <- options(askYesNo = function(...) stop("the askYesNo option was used"))
  on.exit(options(op))
  stopifnot(
    isTRUE(rok:::ask_yes_no("Continue?")),
    identical(rok:::ask_yes_no("Continue?", default = FALSE), FALSE)
  )
})

# Without anyone to ask, an optional question is answered no and the command goes on; with
# rok.yes, yes. Other questions stop.
local({
  needs <- list(id = "env-polars", question = "Set NOT_CRAN?", default = TRUE, optional = TRUE)
  stopifnot(identical(rok:::ask(needs), c("--declined", "env-polars")))
  op <- options(rok.yes = TRUE)
  on.exit(options(op))
  stopifnot(identical(rok:::ask(needs), c("--confirmed", "env-polars")))
  options(rok.yes = NULL)
  needs$optional <- NULL
  stopifnot(inherits(try(rok:::ask(needs), silent = TRUE), "try-error"))
})

# Without a directory, init() writes to the working directory only after asking: without
# anyone to ask, it stops before anything is written.
local({
  err <- tryCatch(rok::init(), error = conditionMessage)
  stopifnot(
    grepl("rok will create a project in", err, fixed = TRUE),
    grepl("init(\".\")", err, fixed = TRUE),
    !file.exists("rok.toml")
  )
})

bin <- Sys.getenv("ROK_TEST_BINARY")
if (nzchar(bin) && file.exists(bin)) {
  home <- tempfile("rok-test-")
  dir.create(home)
  old <- Sys.getenv(c("ROK_DATA_DIR", "ROK_CACHE_DIR"))
  Sys.setenv(ROK_DATA_DIR = file.path(home, "data"), ROK_CACHE_DIR = file.path(home, "cache"))
  op <- options(rok.yes = TRUE, rok.binary = NULL)

  paths <- rok::setup(binary = bin)
  stopifnot(
    file.exists(paths$binary),
    file.exists(file.path(paths$library, "rok", "DESCRIPTION")),
    identical(rok::rok_binary(), normalizePath(paths$binary))
  )

  # A question the binary hands back is answered here (rok.yes) and the command runs again.
  res <- rok:::rok_call(c("r", "list"))
  stopifnot(identical(res$command, "r list"))

  # In the background (interactive sessions), the binary reports its exit status as an event.
  out <- tempfile()
  status <- suppressMessages(rok:::run_watched(paths$binary, c("r", "list", "--json"), out))
  stopifnot(identical(status, 0L), grepl('"r list"', paste(readLines(out), collapse = "")))

  options(op)
  do.call(Sys.setenv, as.list(old))
  unlink(home, recursive = TRUE)
}

# Arguments reach the binary unchanged, even with shell characters and spaces.
if (nzchar(bin) && file.exists(bin) && .Platform$OS.type == "unix") {
  home <- tempfile("rok test ")
  dir.create(home)
  old <- Sys.getenv(c("ROK_DATA_DIR", "ROK_CACHE_DIR"))
  Sys.setenv(ROK_DATA_DIR = file.path(home, "data"), ROK_CACHE_DIR = file.path(home, "cache"))
  op <- options(rok.binary = bin)
  res <- tryCatch(
    rok:::rok_call(c("add", "x", "--version", "< 0.13 & echo injected")),
    error = conditionMessage
  )
  # Outside a project the command fails; what matters is that no shell ran `echo`.
  stopifnot(is.character(res), !file.exists("0.13"))
  options(op)
  do.call(Sys.setenv, as.list(old))
  unlink(home, recursive = TRUE)
}

# run_watched() relays messages, draws the progress line, and returns the exit status the
# program reports; a program that stops without reporting it has failed.
if (.Platform$OS.type == "unix") {
  # `pid = FALSE`: a program that never reports its process id (it is not rok, or has not
  # started yet).
  fake <- function(lines, pid = TRUE) {
    path <- tempfile()
    # The events file is the last argument.
    writeLines(c(
      "#!/bin/sh", "for ev; do :; done",
      if (pid) "printf '\\001pid %s\\n' $$ >> \"$ev\"", lines
    ), path)
    Sys.chmod(path, "755")
    path
  }
  watch <- function(program, start_limit = 120) {
    out <- tempfile()
    shown <- character()
    status <- withCallingHandlers(
      rok:::run_watched(program, character(), out, start_limit = start_limit),
      message = function(m) {
        shown <<- c(shown, conditionMessage(m))
        invokeRestart("muffleMessage")
      }
    )
    list(status = status, shown = shown, out = readLines(out))
  }

  ok <- watch(fake(c(
    "printf '\\001progress   Downloading 2 packages 1/2\\n' >> \"$ev\"",
    "printf '\\001progress \\n' >> \"$ev\"",
    "echo '  Downloaded 2 packages' >> \"$ev\"",
    "echo '{}'",
    "printf '\\001exit 0\\n' >> \"$ev\""
  )))
  stopifnot(
    identical(ok$status, 0L),
    identical(ok$out, "{}"),
    "\r  Downloading 2 packages 1/2" %in% ok$shown,
    "  Downloaded 2 packages\n" %in% ok$shown
  )

  needs <- watch(fake("printf '\\001exit 2\\n' >> \"$ev\""))
  stopifnot(identical(needs$status, 2L))

  crashed <- watch(fake(c("echo 'boom' >&2", "kill -9 $$")))
  stopifnot(identical(crashed$status, 1L), "boom\n" %in% crashed$shown)

  # A program that cannot start says so on stderr: it fails at once, without waiting for the
  # start limit.
  begun <- Sys.time()
  broken <- watch(fake("echo 'cannot run' >&2", pid = FALSE))
  stopifnot(
    identical(broken$status, 1L),
    "cannot run\n" %in% broken$shown,
    difftime(Sys.time(), begun, units = "secs") < 10
  )

  # One that is slow to start is waited for up to the limit, then reported as perhaps still
  # running (not as stopped).
  slow <- watch(fake("sleep 2", pid = FALSE), start_limit = 0.5)
  stopifnot(
    identical(slow$status, 1L),
    any(grepl("did not start within 0.5 seconds", slow$shown, fixed = TRUE))
  )
}

# A downloaded binary is checked against the SHA-256 published beside it; without one, nothing
# is installed. (file:// URLs stand in for the release.)
if (.Platform$OS.type == "unix" && !is.null(rok:::sha256_fun())) local({
  release <- tempfile("release-")
  dir.create(release)
  writeLines("#!/bin/sh", file.path(release, "rok"))
  old <- setwd(release)
  utils::tar("rok-test.tar.gz", files = "rok", compression = "gzip")
  setwd(old)
  archive <- file.path(release, "rok-test.tar.gz")
  url <- paste0("file://", normalizePath(archive))
  sum <- unname(rok:::sha256_fun()(archive))
  get <- function(...) {
    dir <- tempfile("download-")
    dir.create(dir)
    tryCatch(rok:::download_binary(url, dir, ...), error = conditionMessage)
  }

  # No checksum published: refused.
  stopifnot(grepl("Could not get the SHA-256", get(), fixed = TRUE))
  # The right one: the binary is unpacked.
  writeLines(paste0(sum, "  rok-test.tar.gz"), paste0(archive, ".sha256"))
  stopifnot(identical(basename(get()), "rok"))
  # Another one: refused as damaged.
  writeLines(paste0(strrep("0", 64), "  rok-test.tar.gz"), paste0(archive, ".sha256"))
  stopifnot(grepl("is damaged", get(), fixed = TRUE))
  # Where R cannot compute SHA-256, the download goes on with a note.
  shown <- character()
  found <- withCallingHandlers(
    get(sha256 = NULL),
    message = function(m) {
      shown <<- c(shown, conditionMessage(m))
      invokeRestart("muffleMessage")
    }
  )
  stopifnot(identical(basename(found), "rok"), any(grepl("only HTTPS", shown, fixed = TRUE)))
  unlink(release, recursive = TRUE)
})
