# rok <a href="https://github.com/yo5uke/rok"><img src="rpkg/man/figures/logo.png" align="right" height="138" alt="rok logo" /></a>

**A fast package and project manager for R, in the spirit of [uv](https://github.com/astral-sh/uv).**

rok keeps each R project reproducible. A project declares the packages it uses in
`rok.toml`; rok resolves them from a dated snapshot of the
[Posit Package Manager](https://packagemanager.posit.co) (P3M), records the exact versions,
sources, and R version in `rok.lock`, and links them into the project's library from a shared
cache. Anyone who opens the project gets the same packages, and the same R if they need it.

> **Status:** pre-release (0.1.0). Linux and Windows are supported; macOS is experimental.

## Why rok

- **Fast.** Binary packages are unpacked in parallel by rok itself, without starting R,
  and linked from a global cache. A cached sync of the tidyverse takes about 0.1 s.
- **Reproducible as it was.** Versions come from a P3M snapshot date, not from "whatever is
  newest today". GitHub packages are pinned to a commit. The lockfile records the R version
  down to the patch version.
- **R itself, too.** rok installs R versions without administrator rights (Posit's portable
  builds) and moves a project to another R version, choosing package versions that have
  binaries for it.
- **Checked at startup.** When R starts in a project, rok checks the library against
  `rok.lock` (in a few milliseconds) and syncs what is missing.
- **Two front ends, one engine.** The same commands work from R (`rok::add()`) and from a
  terminal (`rok add`).

## Installation

From R, install the package and let it fetch the command-line program:

```r
install.packages(
  "https://github.com/yo5uke/rok/releases/download/v0.1.0/rok_0.1.0.tar.gz",
  repos = NULL, type = "source"
)
rok::setup()   # downloads the rok program for this machine, after asking
```

Or download the program for your machine from the
[releases](https://github.com/yo5uke/rok/releases) (`rok-<target>.tar.gz`), and put `rok`
(`rok.exe` on Windows) on your `PATH`. `rok self update` updates it later.

## Quick start

In R:

```r
# Create a project in the working directory
rok::init()

# Declare, resolve, lock and install packages
rok::add("fixest", "modelsummary")

# Pin GitHub packages to a commit
rok::add("gaborcsardi/praise")

# Check what is out of sync
rok::status()

# Move to the latest snapshot, showing what changes
rok::update()

# Undo the last change
rok::undo()
```

In a terminal:

```sh
# Create a project
rok init my-analysis
cd my-analysis

# Declare, resolve, lock and install packages
rok add fixest modelsummary

# Run a script with the project's R and library
rok run analysis.R
```

Functions are meant to be called as `rok::add()`; `update()` and `remove()` mask
`stats::update()` and `base::remove()` when the package is attached, and transparently
forward calls meant for them.

## Commands

| R | Terminal | What it does |
| --- | --- | --- |
| `init()` | `rok init` | Create a project: `rok.toml`, `rok.lock` and the startup hook |
| `add()` | `rok add` | Declare packages, then resolve, lock and install |
| `remove()` | `rok remove` | Remove packages from the declaration, the lock and the library |
| `sync()` | `rok sync` | Make the library match `rok.lock` |
| `update()` | `rok update` | Move to a newer snapshot date and resolve again |
| `undo()` | `rok undo` | Undo the last `add`, `remove`, `update` or `pin_r` |
| `status()` | `rok status` | Show what is out of sync |
| `why()`, `tree()` | `rok why`, `rok tree` | Show why a package is there, and the dependency tree |
| `run()` | `rok run` | Run a script with the project's R and library |
| `install_r()`, `pin_r()` | `rok r install`, `rok r pin` | Install R; move the project to another R version |
| | `rok r list`, `rok r uninstall` | List and remove R versions |
| `import_renv()`, `export_renv()` | `rok import renv`, `rok export renv` | Migrate from renv; write `renv.lock` |
| | `rok self update` | Update rok |

## Files

| File | Purpose | In Git |
| --- | --- | --- |
| `rok.toml` | What the project declares: packages, version constraints, sources, R version, snapshot date | yes |
| `rok.lock` | The exact resolution: versions, sources, dates, checksums, R version | yes |
| `.Rprofile` | Runs `.rok/activate.R` when R starts | yes |
| `.rok/` | The startup hook (in Git), and the library, undo data and scan cache (not in Git) | partly |

A `rok.toml` looks like this:

```toml
[project]
name = "my-analysis"
r = "4.6"
snapshot = "2026-10-01"

[dependencies]
fixest = "< 0.15"
sf = "*"
praise = { github = "gaborcsardi/praise" }
```

## Coming from renv

In an renv project, `rok::init()` offers to migrate: rok reads `renv.lock`, finds the snapshot
date the versions come from, and reproduces the same versions (rebuilding packages from their
Git commit if a repository no longer has them). `rok export renv` writes a `renv.lock` that
`renv::restore()` can use.

## IDEs

In Positron, `rok::init()` offers to write the project's R to `.vscode/settings.json`, with
paths that start with `~` so that the file is the same for everyone. After `init()`, R
restarts in the project (Positron and RStudio).

## Where rok keeps things

- The command-line program and a copy of the R package: `tools::R_user_dir("rok", "data")`
- Downloaded and built packages, and P3M indexes: `tools::R_user_dir("rok", "cache")`
- R versions rok installs: `~/.local/share/R/rok/r` on Linux,
  `%LOCALAPPDATA%\Programs\R` on Windows, and the data directory on macOS

`ROK_DATA_DIR` and `ROK_CACHE_DIR` can override these locations. rok never needs administrator
rights and does not change user-wide settings; on Linux, when a package or R needs system libraries, it shows the
`apt` command to install them.

## License

MIT
