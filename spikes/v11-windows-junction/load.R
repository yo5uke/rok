# 引数：<ジャンクションのライブラリ> <実体のライブラリ>。それぞれで tidyverse を読み込む時間などを比べる。
args <- commandArgs(TRUE)
run <- function(lib) {
  code <- sprintf(".libPaths(c('%s', .Library)); t <- system.time(suppressPackageStartupMessages(library(tidyverse)))[['elapsed']];
    ok <- all(vapply(rownames(installed.packages(lib.loc = '%s')), requireNamespace, logical(1), quietly = TRUE));
    cat(t, ok, nrow(installed.packages(lib.loc = '%s')), find.package('dplyr'), '\n')",
    gsub("\\\\", "/", lib), gsub("\\\\", "/", lib), gsub("\\\\", "/", lib))
  out <- system2(file.path(R.home("bin"), "Rscript.exe"), c("-e", shQuote(code, type = "cmd")), stdout = TRUE)
  strsplit(out[length(out)], " ")[[1]]
}
for (i in 1:3) {
  j <- run(args[[1]]); d <- run(args[[2]])
  cat(sprintf("%d 回目：library(tidyverse) ジャンクション %s 秒、実体 %s 秒\n", i, j[[1]], d[[1]]))
}
cat("ジャンクション：全パッケージを読み込めた", j[[2]], "、installed.packages() の数", j[[3]], "\n")
cat("find.package('dplyr')：", j[[4]], "\n")
