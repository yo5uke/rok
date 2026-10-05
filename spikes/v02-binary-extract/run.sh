#!/usr/bin/env bash
# V2（Linux 分）：P3M のバイナリを R を使わずに展開し、プロジェクトのライブラリにはリンクだけを置いて読み込めるかを確かめる。
# 前提：spikes/v01-linux-r-install/run.sh を実行済みで、ユーザー領域の R 4.4.2 があること。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out

R_HOME_DIR=${R_HOME_DIR:-$PWD/../v01-linux-r-install/out/r/4.4.2}
R_VER=$("$R_HOME_DIR/bin/Rscript" -e 'cat(format(getRversion()))')
R_MINOR=${R_VER%.*}
DATE=2026-10-01
REPO=https://packagemanager.posit.co/cran/__linux__/noble/$DATE
UA="R ($R_VER x86_64-pc-linux-gnu x86_64 linux-gnu)"
ROOTS="sf data.table R6 cli"
CACHE=out/cache
LIB=out/lib

echo "## 1. 依存の閉包（Depends・Imports。base と R に同梱の推奨パッケージは除く）"
"$R_HOME_DIR/bin/Rscript" -e "
  ap <- available.packages(repos = '$REPO')
  roots <- strsplit('$ROOTS', ' ')[[1]]
  deps <- tools::package_dependencies(roots, db = ap, which = c('Depends', 'Imports'), recursive = TRUE)
  pkgs <- sort(unique(c(roots, unlist(deps))))
  pkgs <- setdiff(pkgs, c('R', rownames(installed.packages(.Library))))
  writeLines(paste(pkgs, ap[pkgs, 'Version']), 'out/pkgs.txt')
"
echo "$(wc -l < out/pkgs.txt) packages: $(cut -d' ' -f1 out/pkgs.txt | tr '\n' ' ')"

echo "## 2. 並列ダウンロード（8並列）"
rm -rf out/dl && mkdir -p out/dl
start=$(date +%s%N)
# sh -c の引数：$0=User-Agent、$1=リポジトリ、$2=パッケージ、$3=版
xargs -P 8 -L 1 sh -c '
  curl -fsSL -A "$0" -D "out/dl/$2.hdr" -o "out/dl/$2_$3.tar.gz" "$1/src/contrib/$2_$3.tar.gz"
' "$UA" "$REPO" < out/pkgs.txt
echo "downloaded in $(( ($(date +%s%N) - start) / 1000000 )) ms, $(du -sh out/dl | cut -f1)"
echo "package types: $(cat out/dl/*.hdr | grep -i '^x-package-type' | tr -d '\r' | sort | uniq -c | tr '\n' ' ')"

echo "## 3. キャッシュへの展開（tar のみ。R は使わない）"
rm -rf "$CACHE" && mkdir -p "$CACHE"
start=$(date +%s%N)
while read -r pkg ver; do
  dest="$CACHE/$pkg/$ver/$R_MINOR-noble"
  mkdir -p "$dest"
  tar -xzf "out/dl/${pkg}_$ver.tar.gz" -C "$dest"
done < out/pkgs.txt
echo "extracted in $(( ($(date +%s%N) - start) / 1000000 )) ms, $(du -sh "$CACHE" | cut -f1)"

echo "## 4. プロジェクトのライブラリにシンボリックリンクを置く"
rm -rf "$LIB" && mkdir -p "$LIB"
while read -r pkg ver; do
  ln -s "$PWD/$CACHE/$pkg/$ver/$R_MINOR-noble/$pkg" "$LIB/$pkg"
done < out/pkgs.txt
ls -l "$LIB" | sed -n 2,3p

echo "## 5. 読み込み"
"$R_HOME_DIR/bin/Rscript" -e "
  .libPaths('$LIB')
  pkgs <- read.table('out/pkgs.txt', col.names = c('pkg', 'ver'))\$pkg
  ok <- vapply(pkgs, function(p) suppressPackageStartupMessages(requireNamespace(p, quietly = TRUE)), logical(1))
  cat('loaded:', sum(ok), '/', length(ok), if (any(!ok)) paste('failed:', pkgs[!ok]), '\n')
  ip <- installed.packages('$LIB')
  cat('installed.packages():', nrow(ip), 'rows\n')
  cat('find.package(\"sf\"):', find.package('sf'), '\n')
  library(sf, quietly = TRUE)
  print(sf_extSoftVersion()[c('GEOS', 'GDAL', 'proj.4')])
  cat('buffer area:', round(as.numeric(st_area(st_buffer(st_point(c(0, 0)), 1))), 3), '\n')
  library(data.table)
  cat('data.table:', data.table(x = 1:10)[, sum(x)], '\n')
"

echo "## 6. R CMD INSTALL で入れた場合との差"
rm -rf out/lib-install && mkdir -p out/lib-install
for pkg in R6 data.table sf; do
  ver=$(awk -v p="$pkg" '$1 == p {print $2}' out/pkgs.txt)
  "$R_HOME_DIR/bin/R" CMD INSTALL --library=out/lib-install "out/dl/${pkg}_$ver.tar.gz" > "out/install-$pkg.log" 2>&1
  if diff -r "$CACHE/$pkg/$ver/$R_MINOR-noble/$pkg" "out/lib-install/$pkg" > "out/diff-$pkg.txt"; then
    echo "$pkg: identical"
  else
    echo "$pkg: differs"; head -5 "out/diff-$pkg.txt"
  fi
done
