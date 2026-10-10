# rok <a href="https://github.com/yo5uke/rok"><img src="man/figures/logo.png" align="right" height="138" alt="rok logo" /></a>

rok is a fast package and project manager for R, in the spirit of
[uv](https://github.com/astral-sh/uv) for Python. A project declares the packages it uses
in `rok.toml`; rok resolves them from a dated snapshot of the
[Posit Package Manager](https://packagemanager.posit.co), records the exact versions in
`rok.lock`, and links them into a project library from a shared cache. The project can then
be reproduced as it was, on Linux, Windows and macOS (experimental).

This package is the R front end of the `rok` command-line program, which does the work. The
first `init()` (or `setup()`) downloads the program from the project's GitHub releases, after
asking.

## Installation

```r
pak::pak("rok")

# or
# install.packages("rok")
```

## Quick start

```r
# Create a project in the working directory
rok::init()

# Declare, resolve, lock and install packages
rok::add("fixest", "sf")

# Pin GitHub packages to a commit
rok::add("gaborcsardi/praise")

# Check what is out of sync
rok::status()

# Move to the latest snapshot, showing what changes
rok::update()

# Undo the last change
rok::undo()
```

Functions are meant to be called as `rok::add()`; `update()` and `remove()` mask
`stats::update()` and `base::remove()` when the package is attached, and transparently
forward calls meant for them.

See <https://github.com/yo5uke/rok> for the command-line program and more.
