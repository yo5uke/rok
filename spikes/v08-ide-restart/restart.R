# V8：base R だけで IDE に R の再起動を指示できるかを調べ、選んだ方法で再起動する。
# 試験用のプロジェクトを開いた IDE のコンソールで source("v08-restart.R") を実行する。
# 再起動の後に source("v08-after.R") を実行すると、結果が out/v08-result.txt に残る。
local({
  ide <- if (nzchar(Sys.getenv("POSITRON"))) {
    paste("Positron", Sys.getenv("POSITRON_VERSION"))
  } else if (nzchar(Sys.getenv("RSTUDIO"))) {
    paste("RStudio", Sys.getenv("RSTUDIO_VERSION"))
  } else {
    "other"
  }
  # Functions that look like a way to restart, in the IDE's environments on the search path.
  tools_envs <- grep("^tools:", search(), value = TRUE)
  found <- unlist(lapply(tools_envs, function(env) {
    funs <- ls(env, all.names = TRUE, pattern = "[Rr]estart")
    if (length(funs)) paste0(env, "::", funs) else character()
  }))
  known <- c(".rs.restartR", ".rs.api.restartSession")
  known <- known[vapply(known, exists, logical(1))]

  dir.create("out", showWarnings = FALSE)
  report <- c(
    paste("time:", format(Sys.time())),
    paste("ide:", ide),
    paste("R:", R.version.string, " pid:", Sys.getpid()),
    paste("search() tools:", paste(tools_envs, collapse = ", ")),
    paste("restart-like functions:", paste(found, collapse = ", ")),
    paste("known functions that exist:", paste(known, collapse = ", "))
  )
  cat(report, sep = "\n")
  cat(report, "", sep = "\n", file = "out/v08-result.txt", append = TRUE)

  if (!length(known)) {
    cat("\nNo known restart function exists here. Note the list above in the record.\n")
    return(invisible())
  }
  pick <- utils::menu(c(known, "Do not restart"), title = "Restart with which function?")
  if (pick == 0L || pick > length(known)) return(invisible())
  method <- known[[pick]]

  # What the next session can compare against.
  assign("v08_marker", "set before the restart", envir = globalenv())
  saveRDS(
    list(pid = Sys.getpid(), time = Sys.time(), method = method, wd = getwd()),
    "out/v08-before.rds"
  )
  cat("\nRestarting with ", method, "() ...\n", sep = "")
  f <- get(method)
  if (method == ".rs.api.restartSession") f(command = "cat('v08: command after restart ran\\n')") else f()
})
