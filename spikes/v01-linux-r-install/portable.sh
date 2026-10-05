#!/usr/bin/env bash
# V1（追記）：Posit の portable な R ビルド（manylinux_2_34、試験的）の予備確認。
# まっさらな ubuntu:24.04 で、一般ユーザーとして展開するだけで R が動くか。P3M のバイナリと組み合わせて動くか。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out/portable
V=${R_VER:-4.4.2}
T=out/portable/R-$V-manylinux_2_34.tar.gz
[ -f "$T" ] || curl -fsS -o "$T" "https://cdn.posit.co/r/manylinux_2_34/R-$V-manylinux_2_34.tar.gz"

echo "## A. apt で何も入れない状態"
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp/home -v "$PWD/out/portable":/p:ro ubuntu:24.04 sh -c "
  mkdir -p /tmp/home/r && tar -xzf /p/R-$V-manylinux_2_34.tar.gz -C /tmp/home/r
  p=/tmp/home/r/$V
  \$p/bin/R --version | head -1
  \$p/bin/Rscript -e 'cat(\"R.home:\", R.home(), \"\\ncapabilities:\", names(which(capabilities())), \"\\n\")'
  echo \"missing libs (R_HOME/lib on the search path): \$(for f in \$p/lib/R/bin/exec/R \$p/lib/R/lib/*.so \$p/lib/R/modules/*.so; do LD_LIBRARY_PATH=\$p/lib/R/lib ldd \$f | grep 'not found'; done | sort -u | wc -l)\"
  \$p/bin/Rscript -e 'cat(\"solve():\", solve(matrix(c(2, 1, 1, 3), 2))[1], \"\\n\")'
"

echo "## B. ca-certificates・libcurl4t64・libgomp1 だけを入れ、P3M のバイナリを入れる"
cat > out/portable/check.R <<'RS'
lib <- file.path(tempdir(), "lib"); dir.create(lib)
install.packages(c("data.table", "curl", "jsonlite"), lib = lib,
                 repos = "https://packagemanager.posit.co/cran/__linux__/noble/2026-10-01", quiet = TRUE)
.libPaths(lib); library(data.table); library(curl); library(jsonlite)
cat("data.table:", data.table(x = 1:3)[, sum(x)], "\n")
cat("curl package libcurl:", curl::curl_version()$version, " R's bundled libcurl:", libcurlVersion(), "\n")
cat("https via curl package:", curl::curl_fetch_memory("https://packagemanager.posit.co/__api__/status")$status_code, "\n")
RS
docker run --rm -v "$PWD/out/portable":/p:ro ubuntu:24.04 sh -c "
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends ca-certificates libcurl4t64 libgomp1 >/dev/null 2>&1
  setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups env HOME=/tmp/home sh -c '
    mkdir -p /tmp/home/r && tar -xzf /p/R-$V-manylinux_2_34.tar.gz -C /tmp/home/r
    /tmp/home/r/$V/bin/Rscript /p/check.R 2>&1 | grep -vE \"^ *$\" | tail -3'
"
