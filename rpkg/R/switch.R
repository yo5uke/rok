# Switching to the project after init() (requirements chapter 5): in an IDE, restart R there
# (or open the project); outside one, activate the project in this session if nothing is
# loaded yet. `.rs.api.restartSession()` restarts R in both Positron and RStudio (V8).

# What to do after init(): "restart", "open", "activate", "advise" or "switch-r".
#   same      the project is the working directory
#   ide       "positron", "rstudio" or "none"
#   loaded    packages loaded besides R's own and rok
#   same_r    the running R has the project's minor version
after_init_action <- function(same, ide, loaded, same_r) {
  if (!same_r) return("switch-r")
  if (ide != "none") return(if (same) "restart" else "open")
  if (length(loaded)) "advise" else "activate"
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
  base_pkgs <- rownames(utils::installed.packages(lib.loc = .Library, priority = "base"))
  ide <- running_ide()
  action <- after_init_action(
    same = identical(root, normalizePath(getwd(), mustWork = FALSE)),
    ide = ide,
    loaded = setdiff(loadedNamespaces(), c(base_pkgs, "rok")),
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
    activate = {
      # The working directory and the library paths change: only when asked.
      ok <- isTRUE(ask_yes_no(paste0(
        "Switch this R session to the project? The working directory becomes ", root, "."
      )))
      if (!ok) {
        message("\u2139 Start R in ", root, " to use the project.")
        return(invisible())
      }
      setwd(root)
      source(file.path(root, ".rok", "activate.R"), local = new.env())
      message("\u2714 Switched to the project: the working directory is ", root, ".")
    },
    advise = message(
      "\u2139 Packages are already loaded in this R session. Restart R in ", root,
      " to use the project."
    )
  )
  invisible()
}
