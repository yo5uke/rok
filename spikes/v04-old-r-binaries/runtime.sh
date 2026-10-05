#!/usr/bin/env bash
# V4（2）：古い日付・古い R の版のバイナリが、今のディストリビューション（Ubuntu 24.04）で実際に動くかを確かめる。
# docker の ubuntu:24.04 に、R の実行時ライブラリ・libpcre3・sf の実行時ライブラリだけを入れる（コンパイラは入れない）。
# 一般ユーザーとして、V1 の方法で R を置き、P3M の noble のスナップショットから install.packages() で入れて読み込む。
# コンパイラがないので、バイナリがなくソースに落ちたコンパイルが要るパッケージは失敗する。
# 前提：v01 の run.sh を実行済み（R の実行時ライブラリの一覧と relocate.sh を使う）。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out
SPIKES=$(cd .. && pwd)
V01=$SPIKES/v01-linux-r-install

# 調べる組：R の版と日付（バイナリの前倒しのビルドや、古い R の版を含める）
COMBOS=${COMBOS:-"3.6.3:2023-06-01 4.2.3:2023-06-01 4.4.3:2020-06-01 4.5.2:2024-10-01 4.6.1:2026-02-02 4.6.1:2026-10-01"}
ROOTS='c("data.table", "jsonlite", "R6", "sf")'

for combo in $COMBOS; do
  v=${combo%%:*}
  t="$V01/out/R-$v-ubuntu-2404.tar.gz"
  [ -f "$t" ] || curl -fsS -o "$t" "https://cdn.posit.co/r/ubuntu-2404/R-$v-ubuntu-2404.tar.gz"
done

cat > out/check.R <<EOF
args <- commandArgs(TRUE)
date <- args[1]
lib <- file.path(tempdir(), "lib"); dir.create(lib)
options(repos = c(CRAN = paste0("https://packagemanager.posit.co/cran/__linux__/noble/", date)), warn = 1)
roots <- $ROOTS
ap <- available.packages()
deps <- unlist(tools::package_dependencies(roots, db = ap, which = c("Depends", "Imports", "LinkingTo"), recursive = TRUE))
pkgs <- setdiff(unique(c(roots, deps)), rownames(installed.packages(.Library)))
suppressWarnings(install.packages(pkgs, lib = lib, quiet = TRUE))
.libPaths(lib)
inst <- installed.packages(lib)
built <- inst[, "Built"]
# Built の日時が今日なら、手元でソースからビルドしたことになる
local <- grepl(format(Sys.Date()), built, fixed = TRUE)
missing <- setdiff(pkgs, rownames(inst))
loaded <- vapply(roots, function(p) isTRUE(suppressPackageStartupMessages(requireNamespace(p, quietly = TRUE))), logical(1))
cat(sprintf("R %s @ %s: %d packages, binary %d, built here %d, not installed %d [%s]; loaded roots: %s\n",
    getRversion(), date, length(pkgs), sum(!local), sum(local), length(missing),
    paste(missing, collapse = " "), paste(names(loaded)[loaded], collapse = " ")))
if (loaded[["sf"]]) cat("  sf:", paste(names(sf::sf_extSoftVersion()[c("GEOS", "GDAL", "proj.4")]), sf::sf_extSoftVersion()[c("GEOS", "GDAL", "proj.4")], collapse = " "), "\n")
EOF

R_RUNTIME=$(awk '{print $2}' "$V01/out/soname-pkg.txt" | sort -u | tr '\n' ' ')
EXTRA="libpcre3 libgdal34t64 libgeos-c1t64 libproj25 libudunits2-0 ca-certificates"
docker run --rm -v "$SPIKES":"$SPIKES" ubuntu:24.04 sh -c "
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $R_RUNTIME $EXTRA >/dev/null 2>&1
  command -v gcc >/dev/null || echo 'no compilers'
  for combo in $COMBOS; do
    v=\${combo%%:*}; d=\${combo#*:}
    setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups env HOME=/tmp/home sh -c \"
      p=\\\$($V01/relocate.sh $V01/out/R-\$v-ubuntu-2404.tar.gz /tmp/home/r)
      \\\$p/bin/Rscript $PWD/out/check.R \$d 2>&1 | grep -E '^(R |  sf)'
    \"
  done
" 2>&1 | tee out/runtime.txt
