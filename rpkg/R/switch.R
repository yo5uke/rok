# Switching to the project after init() (requirements chapter 5): in an IDE, restart R there
# (or open the project); outside one, say where to start R. The running session's working
# directory, options and library paths are left as they are (CRAN policy).
# `.rs.api.restartSession()` restarts R in both Positron and RStudio (V8).

# What to do after init(): "restart", "open", "advise" or "switch-r".
#   same      the project is the working directory
#   ide       "positron", "rstudio" or "none"
#   same_r    the running R has the project's minor version
after_init_action <- function(same, ide, same_r) {
  if (!same_r) return("switch-r")
  if (ide == "none") return("advise")
  if (same) "restart" else "open"
}

# The objects a restart would clear. .Random.seed is not the person's work: Positron's help
# server creates it in every session (tools::startDynamicHelp() draws a random port).
global_objects <- function(env = globalenv()) {
  setdiff(ls(env, all.names = TRUE), ".Random.seed")
}

running_ide <- function() {
  if (nzchar(Sys.getenv("POSITRON"))) "positron" else if (nzchar(Sys.getenv("RSTUDIO"))) "rstudio" else "none"
}

# Calls a function the IDE defines (not part of this package), if it exists.
ide_call <- function(name, ...) {
  if (!exists(name, mode = "function")) return(FALSE)
  do.call(get(name, mode = "function"), list(...))
  TRUE
}

switch_to_project <- function(root, r_version) {
  root <- normalizePath(root, mustWork = FALSE)
  ide <- running_ide()
  same <- identical(root, normalizePath(getwd(), mustWork = FALSE))
  action <- after_init_action(
    same = same,
    ide = ide,
    same_r = identical(sub("^(\\d+\\.\\d+).*$", "\\1", r_version), current_r_minor())
  )
  ide_name <- if (ide == "positron") "Positron" else "RStudio"
  switch(action,
    "switch-r" = message(
      "\u2139 The project uses R ", r_version, ", but this is R ", getRversion(), ".\n",
      "  Start R ", r_version, " in ", root,
      if (ide != "none") paste0(" (choose it in ", ide_name, ")"), " to use the project."
    ),
    restart = {
      n <- length(global_objects())
      ok <- n == 0L || isTRUE(ask_yes_no(paste0(
        "Restart R to start using the project? The ", n, " object", if (n != 1L) "s",
        " in the global environment will be cleared."
      ), default = FALSE))
      if (ok && ide_call(".rs.api.restartSession")) return(invisible())
      message("\u2139 Restart R to start using the project.")
    },
    open = {
      ok <- isTRUE(ask_yes_no(paste0("Open ", root, " in ", ide_name, " now?")))
      opened <- ok && if (ide == "positron") {
        ide_call(".ps.ui.openWorkspace", root, FALSE)
      } else {
        ide_call(".rs.api.openProject", root)
      }
      if (!opened) message("\u2139 Open ", root, " in ", ide_name, " to use the project.")
    },
    advise = message(
      "\u2139 ", if (same) "Restart R" else paste("Start R in", root), " to use the project."
    )
  )
  invisible()
}

# Packages this session loaded before a command re-linked or removed them in its library
# (requirements chapter 5): they stay as loaded until R restarts. Only when the session uses
# that library. `result` is the command's result.
stale_loaded <- function(result, loaded = loadedNamespaces(), libs = .libPaths()) {
  if (!is.list(result) || is.null(result$library)) return(character())
  norm <- function(p) normalizePath(p, winslash = "/", mustWork = FALSE)
  if (!norm(result$library) %in% norm(libs)) return(character())
  sort(intersect(as.character(c(unlist(result$linked), unlist(result$unlinked))), loaded))
}

# Prompts to restart R if the command changed loaded packages. Returns `result` invisibly.
advise_restart <- function(result) {
  pkgs <- stale_loaded(result)
  if (length(pkgs)) {
    message(
      "! This R session still uses ", paste(pkgs, collapse = ", "),
      " as loaded before the change.\n",
      "\u2139 Restart R to use the library as rok.lock records it."
    )
  }
  invisible(result)
}
