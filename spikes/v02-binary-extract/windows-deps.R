# 引数：<P3M の contrib の URL> <出力の CSV> <パッケージ...>
# 指定したパッケージと、その依存（Depends・Imports、再帰的）の名前・版・Hash を書き出す。推奨パッケージは除く。
args <- commandArgs(TRUE)
ap <- available.packages(contriburl = args[[1]], fields = "Hash", filters = list())
want <- args[-(1:2)]
deps <- tools::package_dependencies(want, db = ap, which = c("Depends", "Imports"), recursive = TRUE)
skip <- c("R", rownames(installed.packages(priority = c("base", "recommended"))))
pkgs <- sort(setdiff(unique(c(want, unlist(deps))), skip))
write.csv(data.frame(package = pkgs, version = ap[pkgs, "Version"], hash = ap[pkgs, "Hash"]),
          args[[2]], row.names = FALSE)
