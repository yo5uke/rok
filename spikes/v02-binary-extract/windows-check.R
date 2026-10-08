# 引数：<ライブラリ> <パッケージ...>。そのライブラリと R 本体のライブラリだけで、読み込みと動作を確かめる。
args <- commandArgs(TRUE)
lib <- args[[1]]
.libPaths(c(lib, .Library))
pkgs <- args[-1]
ok <- vapply(pkgs, requireNamespace, logical(1), quietly = TRUE)
cat(sum(ok), "/", length(ok), "パッケージが requireNamespace() で読み込めた\n")
if (!all(ok)) print(pkgs[!ok])
cat("installed.packages() が列挙した数：", nrow(installed.packages(lib.loc = lib)), "\n")
cat("sf のパス：", find.package("sf"), "\n")

suppressPackageStartupMessages(library(sf))
print(sf_extSoftVersion()[c("GEOS", "GDAL", "proj.4")])
pt <- st_sfc(st_point(c(139.7, 35.7)), crs = 4326)
cat("sf：バッファの面積（GEOS）", round(as.numeric(st_area(st_buffer(st_transform(pt, 3857), 1000)))), "\n")
gj <- '{"type":"Point","coordinates":[1,2]}'
cat("sf：GeoJSON の読み込み（GDAL）", nrow(st_read(gj, quiet = TRUE)), "行\n")

library(data.table)
setDTthreads(4)
dt <- data.table(g = rep(1:3, 1e5), x = seq_len(3e5))
print(dt[, .(m = mean(x)), by = g])
cat("data.table のスレッド：", getDTthreads(), "\n")
