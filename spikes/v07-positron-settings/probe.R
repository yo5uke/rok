# V7：今のコンソールの R を記録する。IDE のコンソールで source("v07-probe.R") を実行する。
# 結果は画面に出し、out/v07-probe.txt にも追記する。
local({
  ide <- if (nzchar(Sys.getenv("POSITRON"))) {
    paste("Positron", Sys.getenv("POSITRON_VERSION"))
  } else if (nzchar(Sys.getenv("RSTUDIO"))) {
    "RStudio"
  } else {
    "other"
  }
  lines <- c(
    paste("time:", format(Sys.time())),
    paste("ide:", ide),
    paste("R:", R.version.string),
    paste("R.home():", R.home()),
    paste("libPaths:", paste(.libPaths(), collapse = " | ")),
    paste("settings.json:", if (file.exists(".vscode/settings.json")) paste(readLines(".vscode/settings.json"), collapse = " ") else "(none)"),
    ""
  )
  cat(lines, sep = "\n")
  dir.create("out", showWarnings = FALSE)
  cat(lines, sep = "\n", file = "out/v07-probe.txt", append = TRUE)
})
