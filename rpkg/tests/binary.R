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
