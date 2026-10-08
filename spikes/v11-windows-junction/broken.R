# 引数：<ライブラリ>。リンク先のない R6 のジャンクションが、R からどう見えるか。
lib <- commandArgs(TRUE)[[1]]
p <- file.path(lib, "R6")
cat("R：file.exists", file.exists(p), "、dir.exists", dir.exists(p),
    "、Sys.readlink", shQuote(Sys.readlink(p)), "\n")
ip <- rownames(installed.packages(lib.loc = lib))
cat("installed.packages() に R6 が含まれるか：", "R6" %in% ip, "（全", length(ip), "件）\n")
cat("requireNamespace('R6')：", requireNamespace("R6", lib.loc = lib, quietly = TRUE), "\n")
