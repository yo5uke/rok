#!/usr/bin/env bash
# V1（ホスト側）：Posit の R ビルドをユーザー領域に置き、パッケージの導入と読み込みを確かめる。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out

R_VER=${R_VER:-4.4.2}
OS_ID=ubuntu-2404
tarball="out/R-$R_VER-$OS_ID.tar.gz"

echo "## 入手可能な版（cdn.posit.co/r/versions.json）"
curl -fsS https://cdn.posit.co/r/versions.json -o out/versions.json
jq -r '"\(.r_versions | length) versions: \(.r_versions[:6] | join(" ")) ..."' out/versions.json

echo "## 取得と展開"
[ -f "$tarball" ] || curl -fsS -o "$tarball" "https://cdn.posit.co/r/$OS_ID/R-$R_VER-$OS_ID.tar.gz"
start=$(date +%s%N)
prefix=$(./relocate.sh "$tarball" out/r)
echo "relocated to $prefix in $(( ($(date +%s%N) - start) / 1000000 )) ms"

echo "## R と Rscript"
"$prefix/bin/R" --version | head -1
"$prefix/bin/Rscript" -e 'cat("R.home:", R.home(), "\n")'
"$prefix/bin/R" --no-echo -e '
  out <- system2(file.path(R.home("bin"), "Rscript"), c("-e", shQuote("cat(R.version.string)")), stdout = TRUE)
  cat("Rscript from R.home(\"bin\"):", out, "\n")
  caps <- capabilities()[c("jpeg", "png", "tiff", "tcltk", "X11", "cairo", "ICU", "libcurl")]
  cat("capabilities:", paste0(names(caps), "=", caps), "\n")'

echo "## ソースからのビルド（cli）"
cli_ver=$(curl -fsS https://cloud.r-project.org/src/contrib/PACKAGES |
  awk 'BEGIN{RS=""} /(^|\n)Package: cli\n/' | awk -F': ' '/^Version/{print $2}')
[ -f "out/cli_$cli_ver.tar.gz" ] ||
  curl -fsS -o "out/cli_$cli_ver.tar.gz" "https://cloud.r-project.org/src/contrib/cli_$cli_ver.tar.gz"
rm -rf out/lib-src && mkdir -p out/lib-src
"$prefix/bin/R" CMD INSTALL --library=out/lib-src "out/cli_$cli_ver.tar.gz" > out/cli-install.log 2>&1
tail -1 out/cli-install.log
"$prefix/bin/Rscript" -e '.libPaths("out/lib-src"); library(cli); cat("Built:", packageDescription("cli")$Built, "\n")'

echo "## 他の版の書き換え（起動の可否と、足りない共有ライブラリ）"
for v in ${OTHER_VERSIONS:-4.6.1 3.6.3}; do
  t="out/R-$v-$OS_ID.tar.gz"
  [ -f "$t" ] || curl -fsS -o "$t" "https://cdn.posit.co/r/$OS_ID/R-$v-$OS_ID.tar.gz"
  p=$(./relocate.sh "$t" out/r)
  missing=$(LD_LIBRARY_PATH="$p/lib/R/lib" ldd "$p/lib/R/bin/exec/R" "$p/lib/R/lib/libR.so" |
    awk '/not found/ {print $1}' | sort -u | tr '\n' ' ')
  if [ -n "$missing" ]; then
    echo "$v: relocated, but missing: $missing"
  else
    echo "$v: $("$p/bin/Rscript" -e 'cat(R.version.string)')"
  fi
done
