# 引数：<試験用のライブラリ> <P3M の日付>。この R で、起動・HTTPS・バイナリの導入・ソースからのビルドを確かめる。
args <- commandArgs(TRUE)
lib <- args[[1]]
dir.create(lib, showWarnings = FALSE)
.libPaths(c(lib, .Library))
cat(R.version.string, "\n")
cat("R.home()：", R.home(), "\n")
cat(".libPaths()：", paste(.libPaths(), collapse = " ; "), "\n")
cap <- capabilities()
cat("capabilities：", paste(names(cap)[cap], collapse = " "), "\n")

versions <- tryCatch(readLines(url("https://cdn.posit.co/r/versions.json"), warn = FALSE),
                     error = function(e) conditionMessage(e))
cat("HTTPS（url()）：", if (length(versions) && grepl("r_versions", versions[[1]])) "動く" else versions, "\n")

repo <- sprintf("https://packagemanager.posit.co/cran/%s", args[[2]])
t <- system.time(utils::install.packages(c("jsonlite", "data.table"), lib = lib, repos = repo,
                                         type = "binary", quiet = TRUE))
library(data.table, lib.loc = lib)
cat("バイナリ：jsonlite・data.table を", round(t[["elapsed"]], 1), "秒で導入。data.table",
    format(packageVersion("data.table", lib.loc = lib)), "が動く：",
    data.table(x = 1:3)[, sum(x)] == 6, "\n")

# Rtools は R の etc/Rcmd_environ が参照する（RTOOLS45_HOME、既定は c:/rtools45）
rcmd <- readLines(file.path(R.home("etc"), "Rcmd_environ"))
cat("Rcmd_environ：", grep("RTOOLS", rcmd, value = TRUE)[1], "\n")
t <- system.time(res <- tryCatch({
  utils::install.packages("cli", lib = lib, repos = repo, type = "source", quiet = TRUE)
  requireNamespace("cli", lib.loc = lib, quietly = TRUE)
}, warning = function(w) conditionMessage(w), error = function(e) conditionMessage(e)))
cat("ソースからのビルド（cli、C）：", if (isTRUE(res)) "成功" else res, "（", round(t[["elapsed"]], 1), "秒）\n")
