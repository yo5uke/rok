# rok (development version)

* Functions called from a subfolder of a project (R's working directory below the project
  root) find the project again; they failed with "No rok.toml found".
* `status()` returns the details of each problem: `problems$details` is a list column
  (it was `NA` unless a problem had exactly one detail).
* `options(rok.yes = TRUE)` also answers the questions the rok program asks on a terminal,
  and applies to `run()`.
* An argument the rok program does not accept, such as `pin_r(strategy = "x")`, fails at
  once in interactive sessions instead of after 10 seconds.
* `import_renv()` stops with a clear message for a GitHub package without a full commit SHA.
* `run()` asks in R what syncing the project needs an answer to (installing the project's R,
  building packages from source), instead of stopping with advice meant for a terminal.
* `setup()` installs a downloaded binary only once it matches the SHA-256 published with it,
  and says so when it cannot get one. R older than 4.5.0 cannot compute SHA-256: there it
  says that only HTTPS protected the download.
* `init()` that stops before the project is written (a question answered no, an error)
  leaves no empty directory behind.
* When the rok program is slow to start (a virus scanner checking it), interactive sessions
  wait for it, up to 2 minutes, instead of reporting a failure after 10 seconds while it
  went on in the background.

# rok 0.2.0

* Without a directory, `init()` asks before creating the project in the working directory;
  the first time, the same question covers installing the rok program.
* `install_r()` and `rok r install` ask before installing R, saying which version and where
  it goes; `--yes` (or `options(rok.yes = TRUE)` in R) answers yes.
* Outside Positron and RStudio, `init()` no longer switches the running R session to the
  project; it says to start R in the project folder.
* `add()` takes `repo`, the URL of a CRAN-like repository such as R-multiverse or an
  r-universe; the repository is declared in `rok.toml` under a name taken from the URL.
* Known build-time environment variables, such as `NOT_CRAN = "true"` for polars, are offered
  by `add()` before installing and suggested by `status()`.
* After `add()`, `remove()`, `sync()`, `update()`, `undo()` and `pin_r()`, packages that were
  loaded in the session and changed are named, with a prompt to restart R.
* `status()` lists unmanaged packages even when `rok.lock` records them, and warns about those
  not installed. Unmanaged packages installed or updated later are recorded in `rok.lock` by
  `sync()` and when R starts.
* Scripts run with `run()` can load rok itself.
* Locked GitHub packages keep their version constraints when the project is resolved again.
* Builds from source allow downloads to take up to 10 minutes, so that packages that download
  a large prebuilt library (polars) do not fall back to compiling it.
* Branches, tags and default branches of GitHub packages are resolved with the reference list
  Git itself reads, not GitHub's REST API, whose limit of 60 requests an hour without a token
  is often used up on shared addresses such as CI runners. A token GitHub does not accept no
  longer breaks public repositories.
* Errors from GitHub's API say whether a rate limit was reached, and a short secondary limit is
  waited out once.

# rok 0.1.0

* First release.
* Projects: `init()`, `add()`, `remove()`, `sync()`, `update()`, `undo()`, `status()`, `why()`
  and `tree()`, with packages resolved from dated snapshots of the Posit Package Manager and
  recorded in `rok.lock`.
* R itself: `install_r()` and `pin_r()` install R versions without administrator rights
  (Linux and Windows; macOS is experimental) and move a project to another R version.
* Projects are checked and synced when R starts in them, through `.rok/activate.R`.
* Migration from renv: `import_renv()` and `export_renv()`.
* Downloads show their progress: one line in the R console, bars on a terminal.
* `setup()` downloads the matching `rok` command-line program from the project's GitHub
  releases, after asking.
