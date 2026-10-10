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

bin <- Sys.getenv("ROK_TEST_BINARY")
if (nzchar(bin) && file.exists(bin)) {
  home <- tempfile("rok-test-")
  dir.create(home)
  old <- Sys.getenv(c("ROK_DATA_DIR", "ROK_CACHE_DIR"))
  Sys.setenv(ROK_DATA_DIR = file.path(home, "data"), ROK_CACHE_DIR = file.path(home, "cache"))
  options(rok.yes = TRUE, rok.binary = NULL)

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

  do.call(Sys.setenv, as.list(old))
  unlink(home, recursive = TRUE)
}

# Arguments reach the binary unchanged, even with shell characters and spaces.
if (nzchar(bin) && file.exists(bin) && .Platform$OS.type == "unix") {
  home <- tempfile("rok test ")
  dir.create(home)
  Sys.setenv(ROK_DATA_DIR = file.path(home, "data"), ROK_CACHE_DIR = file.path(home, "cache"))
  options(rok.binary = bin)
  res <- tryCatch(
    rok:::rok_call(c("add", "x", "--version", "< 0.13 & echo injected")),
    error = conditionMessage
  )
  # Outside a project the command fails; what matters is that no shell ran `echo`.
  stopifnot(is.character(res), !file.exists("0.13"))
  unlink(home, recursive = TRUE)
}

# run_watched() relays messages, draws the progress line, and returns the exit status the
# program reports; a program that stops without reporting it has failed.
if (.Platform$OS.type == "unix") {
  fake <- function(lines) {
    path <- tempfile()
    # The events file is the last argument.
    writeLines(c(
      "#!/bin/sh", "for ev; do :; done", "printf '\\001pid %s\\n' $$ >> \"$ev\"", lines
    ), path)
    Sys.chmod(path, "755")
    path
  }
  watch <- function(program) {
    out <- tempfile()
    shown <- character()
    status <- withCallingHandlers(
      rok:::run_watched(program, character(), out),
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
}
