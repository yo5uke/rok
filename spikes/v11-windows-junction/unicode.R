# 引数：<ライブラリ>（日本語を含むパス。中の cli は、日本語と空白を含むパスへのジャンクション）
lib <- commandArgs(TRUE)[[1]]
.libPaths(c(lib, .Library))
ok <- requireNamespace("cli", quietly = TRUE)
cat("日本語と空白を含むパスのジャンクション経由で cli を読み込めたか：", ok, "\n")
if (ok) cat("find.package('cli')：", find.package("cli"), "、Encoding：", Encoding(find.package("cli")), "\n")
