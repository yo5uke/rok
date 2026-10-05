#!/usr/bin/env bash
# V1（まっさらな環境）：ubuntu:24.04 で、一般ユーザー（uid 1000）として R をユーザー領域に置く。
#   A. 何も足さない状態で、足りない共有ライブラリを調べる
#   B. 実行時ライブラリだけを apt で入れ（コンパイラや -dev は入れない）、P3M のバイナリを導入・読み込む
# 前提：run.sh を先に実行して、out/ に R の tar.gz があること。ホストに docker があること。
set -euo pipefail
cd "$(dirname "$0")"

R_VER=${R_VER:-4.4.2}
IMG=ubuntu:24.04
TARBALL=/spike/out/R-$R_VER-ubuntu-2404.tar.gz
DEST=/tmp/home/.local/share/R/rok/r

echo "## A. 何も足さない状態"
docker run --rm --user 1000:1000 -e HOME=/tmp/home -v "$PWD":/spike:ro "$IMG" sh -c "
  p=\$(/spike/relocate.sh $TARBALL $DEST)
  \$p/bin/R --version 2>&1 | head -1
  echo 'missing sonames:'
  for f in \$p/lib/R/bin/exec/R \$p/lib/R/lib/*.so \$p/lib/R/modules/*.so \$p/lib/R/library/*/libs/*.so; do
    LD_LIBRARY_PATH=\$p/lib/R/lib ldd \$f 2>/dev/null
  done | awk '/not found/{print \$1}' | sort -u
" | tee out/container-bare.txt

# 足りない soname を、ホストの dpkg で apt のパッケージ名に対応づける（ホストには全部入っている前提）
ldconfig -p > out/host-ldcache.txt
sed -n '/^missing sonames:/,$p' out/container-bare.txt | tail -n +2 | while read -r so; do
  path=$(awk -v s="$so" '$1 == s && /x86-64/ {print $NF; exit}' out/host-ldcache.txt)
  echo "$so $(dpkg -S "$(readlink -f "$path")" 2>/dev/null | head -1 | cut -d: -f1)"
done > out/soname-pkg.txt
pkgs=$(awk '{print $2}' out/soname-pkg.txt | sort -u | tr '\n' ' ')
echo "runtime packages: $pkgs"

echo "## B. 実行時ライブラリだけを入れた状態"
cat > out/check-binary.R <<'EOF'
lib <- file.path(tempdir(), "lib")
dir.create(lib)
options(repos = c(CRAN = "https://packagemanager.posit.co/cran/__linux__/noble/2026-10-01"))
install.packages(c("R6", "data.table"), lib = lib, quiet = TRUE)
.libPaths(lib)
library(data.table)
library(R6)
cat("data.table Built:", packageDescription("data.table")$Built, "\n")
cat("sum:", data.table(x = 1:5)[, sum(x)], "\n")
cat("capabilities:", names(which(capabilities())), "\n")
EOF
docker run --rm -v "$PWD":/spike:ro "$IMG" sh -c "
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $pkgs ca-certificates >/dev/null 2>&1
  command -v gcc >/dev/null || echo 'no compilers'
  setpriv --reuid=1000 --regid=1000 --clear-groups env HOME=/tmp/home sh -c '
    echo uid=\$(id -u)
    p=\$(/spike/relocate.sh $TARBALL $DEST)
    \$p/bin/Rscript /spike/out/check-binary.R
  '
" | tee out/container-runtime.txt
