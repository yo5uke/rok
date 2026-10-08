# V8：再起動の後に source("v08-after.R") を実行し、何が起きたかを記録する。
local({
  if (!file.exists("out/v08-before.rds")) {
    cat("out/v08-before.rds がありません。先に source(\"v08-restart.R\") で再起動してください。\n")
    return(invisible())
  }
  before <- readRDS("out/v08-before.rds")
  log <- if (file.exists("out/startup.log")) readLines("out/startup.log") else character()
  started_after <- Filter(function(l) {
    parts <- strsplit(l, "\t", fixed = TRUE)[[1L]]
    length(parts) >= 2L && parts[[2L]] == as.character(Sys.getpid())
  }, log)
  lines <- c(
    paste("method:", before$method),
    paste("new process:", Sys.getpid() != before$pid, sprintf("(pid %s -> %s)", before$pid, Sys.getpid())),
    paste("startup hook (.Rprofile) ran in the new session:", length(started_after) > 0L),
    paste("working directory kept:", identical(getwd(), before$wd), sprintf("(%s)", getwd())),
    paste("global variable survived the restart:", exists("v08_marker", envir = globalenv())),
    paste("project library first in .libPaths():", grepl("/.rok/library/", .libPaths()[[1L]], fixed = TRUE)),
    ""
  )
  cat(lines, sep = "\n")
  cat(lines, sep = "\n", file = "out/v08-result.txt", append = TRUE)
  unlink("out/v08-before.rds")
})
