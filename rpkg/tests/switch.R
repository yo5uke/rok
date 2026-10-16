# What init() does afterwards (no IDE here, so only the decision is tested).
after_init_action <- rok:::after_init_action
stopifnot(
  identical(after_init_action(TRUE, "positron", character(), TRUE), "restart"),
  identical(after_init_action(FALSE, "rstudio", "dplyr", TRUE), "open"),
  identical(after_init_action(TRUE, "none", character(), TRUE), "activate"),
  identical(after_init_action(FALSE, "none", character(), TRUE), "activate"),
  identical(after_init_action(TRUE, "none", "dplyr", TRUE), "advise"),
  identical(after_init_action(TRUE, "positron", character(), FALSE), "switch-r"),
  identical(after_init_action(TRUE, "none", character(), FALSE), "switch-r")
)

# Before restarting, only the person's objects count: not .Random.seed, which the IDE's help
# server creates in every session.
local({
  env <- new.env()
  assign(".Random.seed", 1L, envir = env)
  stopifnot(identical(rok:::global_objects(env), character()))
  assign("fit", 1, envir = env)
  assign(".hidden", 1, envir = env)
  stopifnot(identical(sort(rok:::global_objects(env)), c(".hidden", "fit")))
})

# After add(), update() and the like: the loaded packages they changed, if the session uses
# the library they changed.
local({
  lib <- tempfile("lib")
  dir.create(lib)
  res <- list(library = lib, linked = list("fixest", "R6"), unlinked = list("old"))
  stale <- function(...) rok:::stale_loaded(res, ...)
  stopifnot(
    identical(stale(c("base", "fixest", "old"), c(lib, .Library)), c("fixest", "old")),
    identical(stale(c("base", "fixest"), .Library), character()),
    identical(stale("base", lib), character()),
    identical(rok:::stale_loaded(list(command = "update", dry_run = TRUE)), character())
  )
})
