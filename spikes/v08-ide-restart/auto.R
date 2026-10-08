# V8（自動）：IDE がセッションを始めたときのフックで、base R から再起動を指示し、何が起きたかを記録する。
# 試験用のプロジェクトの .Rprofile が、環境変数 ROK_V08_AUTO があるときだけ読む（ide-auto.ps1 が設定する）。
# 1回目のセッション：グローバル変数を置き、.rs.api.restartSession() で再起動を指示する
# 2回目のセッション：プロセス・作業ディレクトリ・グローバル変数・ライブラリを記録する
# 結果は out/v08-auto.txt に追記する。
local({
  dir.create("out", showWarnings = FALSE)
  ide <- if (nzchar(Sys.getenv("POSITRON"))) "Positron" else if (nzchar(Sys.getenv("RSTUDIO"))) "RStudio" else "other"
  record <- function(...) {
    cat(format(Sys.time(), "%H:%M:%OS3"), ide, ..., "\n", sep = "\t", file = "out/v08-auto.txt", append = TRUE)
  }
  on_init <- function(kind) {
    record(kind, paste0("pid=", Sys.getpid()), paste0("wd=", getwd()),
           paste0("marker=", exists("v08_marker", envir = globalenv())),
           paste0("lib=", .libPaths()[[1L]]),
           paste0("restartSession=", exists(".rs.api.restartSession")),
           paste0("R=", getRversion()))
    if (!file.exists("out/v08-auto.restarted")) {
      file.create("out/v08-auto.restarted")
      assign("v08_marker", TRUE, envir = globalenv())
      record("restart-requested")
      tryCatch(.rs.api.restartSession(), error = function(e) record("restart-failed", conditionMessage(e)))
    }
  }
  setHook("positron.session_init", function(start_type) on_init(start_type))
  setHook("rstudio.sessionInit", function(newSession) on_init(if (isTRUE(newSession)) "new" else "resumed"))
})
