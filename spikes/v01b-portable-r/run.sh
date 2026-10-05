#!/usr/bin/env bash
# V1b（Linux 分）：Posit の portable な R ビルド（manylinux_2_34）を確かめる。
#   A. 版を変えても、apt で何も入れずに展開するだけで動くか（3.6.3、4.4.2、4.6.1）
#   C. システムのライブラリを使うパッケージ（P3M のバイナリ）と組み合わせて動くか
#      portable な R は、OpenSSL・libcurl・libxml2・libgomp・freetype などを別名で同梱している。
#      同じ記号を持つシステムのライブラリが同じプロセスに入っても動くかを、実際の処理で確かめる。
#   （B. ソースからのビルドは、V3c の run.sh で、portable な R を使って確かめる）
# docker の ubuntu:24.04 で、一般ユーザー（ホストと同じ uid）として実行する。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out/dl
VERSIONS=${VERSIONS:-"3.6.3 4.4.2 4.6.1"}
DATE=2026-10-01

for v in $VERSIONS; do
  t=out/dl/R-$v-manylinux_2_34.tar.gz
  [ -f "$t" ] || curl -fsS -o "$t" "https://cdn.posit.co/r/manylinux_2_34/R-$v-manylinux_2_34.tar.gz"
done

echo "## A. apt で何も入れない状態（版ごと）"
for v in $VERSIONS; do
  docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp/home -v "$PWD/out/dl":/dl:ro ubuntu:24.04 sh -c "
    mkdir -p /tmp/home/r && tar -xzf /dl/R-$v-manylinux_2_34.tar.gz -C /tmp/home/r && p=/tmp/home/r/$v
    missing=\$(for f in \$p/lib/R/bin/exec/R \$p/lib/R/lib/*.so \$p/lib/R/modules/*.so \$p/lib/R/library/*/libs/*.so; do
      LD_LIBRARY_PATH=\$p/lib/R/lib ldd \$f | grep 'not found'; done | sort -u | wc -l)
    \$p/bin/Rscript -e 'x <- system2(file.path(R.home(\"bin\"), \"Rscript\"), c(\"-e\", shQuote(\"cat(1)\")), stdout = TRUE)
      cat(R.version.string, \"| missing libs:\", '\$missing', \"| solve:\", solve(matrix(c(2, 1, 1, 3), 2))[1],
          \"| child Rscript:\", x, \"| tcltk:\", capabilities(\"tcltk\"), \"| cairo:\", capabilities(\"cairo\"), \"\\n\")'
  "
done

echo "## C. システムのライブラリを使うパッケージとの組み合わせ（R 4.4.2、P3M $DATE のバイナリ）"
cat > out/check-syslibs.R <<EOF
lib <- file.path(tempdir(), "lib"); dir.create(lib)
options(repos = c(CRAN = "https://packagemanager.posit.co/cran/__linux__/noble/$DATE"), warn = 1)
install.packages(c("data.table", "curl", "openssl", "xml2", "sf", "ragg"), lib = lib, quiet = TRUE)
.libPaths(lib)
ok <- function(label, expr) {
  r <- tryCatch({ force(expr); "ok" }, error = function(e) paste("ERROR:", conditionMessage(e)))
  cat(sprintf("  %-34s %s\n", label, r))
}
ok("R url() over HTTPS (bundled curl)", readLines(url("https://packagemanager.posit.co/__api__/status"), n = 1, warn = FALSE))
ok("data.table with 4 OpenMP threads", { data.table::setDTthreads(4); stopifnot(data.table::data.table(g = rep(1:4, 1e5), x = 1)[, sum(x), by = g][, all(V1 == 1e5)]) })
ok("curl package HTTPS (system curl)", stopifnot(curl::curl_fetch_memory("https://cdn.posit.co/r/versions.json")\$status_code == 200))
ok("openssl sha256 + RSA (system ssl)", { openssl::sha256("rok"); openssl::rsa_keygen(2048) })
ok("xml2 parse + XPath (system xml2)", stopifnot(length(xml2::xml_find_all(xml2::read_xml("<a><b/><b/></a>"), "//b")) == 2))
ok("sf GDAL + PROJ + GEOS", {
  g <- sf::st_read('{"type":"Point","coordinates":[139.7,35.7]}', quiet = TRUE)
  sf::st_buffer(sf::st_transform(g, 3857), 100)
})
ok("ragg PNG + text (system freetype)", { f <- tempfile(fileext = ".png"); ragg::agg_png(f); plot(1:10, main = "rok"); dev.off(); stopifnot(file.size(f) > 0) })
cat("  versions: curl pkg", curl::curl_version()\$version, "/ R's libcurl", libcurlVersion(),
    "/ openssl pkg", openssl::openssl_config()\$version, "\n")
EOF
RUNTIME="ca-certificates libgomp1 libcurl4t64 libxml2 libgdal34t64 libgeos-c1t64 libproj25 libudunits2-0 libfreetype6 libpng16-16t64 libtiff6 libjpeg-turbo8 libwebp7 libharfbuzz0b libfribidi0 libwebpmux3"
docker run --rm -v "$PWD/out":/o:ro ubuntu:24.04 sh -c "
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $RUNTIME >/dev/null 2>&1
  setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups env HOME=/tmp/home sh -c '
    mkdir -p /tmp/home/r && tar -xzf /o/dl/R-4.4.2-manylinux_2_34.tar.gz -C /tmp/home/r
    /tmp/home/r/4.4.2/bin/Rscript /o/check-syslibs.R 2>&1 | grep -E \"^  \"'
"
