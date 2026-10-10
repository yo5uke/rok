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
