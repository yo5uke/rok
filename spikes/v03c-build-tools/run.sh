#!/usr/bin/env bash
# V3c：Linux のビルドツールの不足の判定。V1b（portable な R でソースからビルドできるか）も兼ねる。
# まっさらな ubuntu:24.04（ca-certificates だけを入れる）で、portable な R 4.4.2 を一般ユーザーとして使う。
#   0. 何も入れずにソースからビルドし、失敗のログから不足を読み取れるか（失敗時の案内）
#   1. detect.sh で不足を判定する
#   2. 提案されたものだけを apt で入れる（root）
#   3. もう一度判定し、C（cli・rlang）・C++（Rcpp）・Fortran（quadprog）・システムのライブラリ（xml2）をソースからビルドする
# 前提：v01b の run.sh を実行済み（portable な R の tar.gz を使う）。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out/src
SPIKES=$(cd .. && pwd)
PKGS="cli Rcpp quadprog rlang xml2"   # rlang は xml2 の依存
R_TGZ=$SPIKES/v01b-portable-r/out/dl/R-4.4.2-manylinux_2_34.tar.gz

# ソースの tar.gz（CRAN の現行の版）と、P3M の sysreqs をホストで取得しておく（コンテナには curl がない）
curl -fsS https://cloud.r-project.org/src/contrib/PACKAGES > out/PACKAGES
for p in $PKGS; do
  v=$(awk -v p="$p" 'BEGIN{RS=""} $0 ~ "(^|\n)Package: "p"\n"' out/PACKAGES | awk -F': ' '/^Version/{print $2}')
  [ -f "out/src/${p}_$v.tar.gz" ] || curl -fsS -o "out/src/${p}_$v.tar.gz" "https://cloud.r-project.org/src/contrib/${p}_$v.tar.gz"
done
q=$(echo $PKGS | sed 's/[^ ][^ ]*/pkgname=&/g; s/ /\&/g')
curl -fsS "https://packagemanager.posit.co/__api__/repos/cran/sysreqs?all=false&$q&distribution=ubuntu&release=24.04" |
  jq -r '.requirements[] | .name as $n | .requirements.packages[] | "\($n) \(.)"' > out/sysreqs.txt
echo "P3M sysreqs: $(tr '\n' ';' < out/sysreqs.txt)"

# 失敗のログから不足を読み取る（判定が外れたときの案内に使う）
cat > out/classify.sh <<'EOF'
#!/bin/sh
# ビルドのログから、不足しているものの手がかりを取り出す
grep -hoE "(gcc|g\+\+|gfortran|make|cc|c\+\+): (command )?not found|[A-Za-z0-9_/.+-]+\.h: No such file or directory|Package '?[A-Za-z0-9.+-]+'? was not found in the pkg-config search path|pkg-config: not found|cannot find -l[A-Za-z0-9_+-]+" "$@" | sort -u | sed 's/^/  log: /'
EOF
chmod +x out/classify.sh

U="setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups env HOME=/tmp/home"
docker run --rm -v "$PWD":/w -v "$R_TGZ":/R.tar.gz:ro ubuntu:24.04 sh -c "
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends ca-certificates >/dev/null 2>&1
  $U sh -c 'mkdir -p /tmp/home/r && tar -xzf /R.tar.gz -C /tmp/home/r'
  R=/tmp/home/r/4.4.2/bin/R

  echo '## 0. 何も入れずにソースからビルド（失敗するはず）'
  $U sh -c \"mkdir -p /tmp/home/lib0; for t in /w/out/src/cli_*.tar.gz /w/out/src/xml2_*.tar.gz; do \$R CMD INSTALL --library=/tmp/home/lib0 \\\$t > /tmp/home/build0-\\\$(basename \\\$t).log 2>&1 && echo \\\"  \\\$(basename \\\$t): ok\\\" || echo \\\"  \\\$(basename \\\$t): failed\\\"; done; /w/out/classify.sh /tmp/home/build0-*.log\"

  echo '## 1. 判定'
  $U env DETECT_OUT=/tmp/home/suggest.txt /w/detect.sh \$R /w/out/sysreqs.txt /w/out/src/*.tar.gz
  suggest=\$(cat /tmp/home/suggest.txt)

  echo \"## 2. 提案されたものだけを入れる：\$suggest\"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \$suggest >/dev/null 2>&1

  echo '## 3. もう一度判定し、ソースからビルドする'
  $U /w/detect.sh \$R /w/out/sysreqs.txt /w/out/src/*.tar.gz | sed -n '2,3p'
  $U sh -c \"mkdir -p /tmp/home/lib; for t in /w/out/src/*.tar.gz; do start=\\\$(date +%s); \$R CMD INSTALL --library=/tmp/home/lib \\\$t > /tmp/home/build-\\\$(basename \\\$t).log 2>&1 && r=ok || r=failed; echo \\\"  \\\$(basename \\\$t): \\\$r (\\\$((\\\$(date +%s) - start)) s)\\\"; done
    /tmp/home/r/4.4.2/bin/Rscript -e '.libPaths(\\\"/tmp/home/lib\\\"); for (p in c(\\\"cli\\\", \\\"Rcpp\\\", \\\"quadprog\\\", \\\"xml2\\\")) library(p, character.only = TRUE)
      cat(\\\"  loaded all; quadprog:\\\", quadprog::solve.QP(diag(2), c(1, 1), diag(2), c(0, 0))\\\$value, \\\"| xml2:\\\", xml2::xml_name(xml2::read_xml(\\\"<rok/>\\\")), \\\"\\\\n\\\")'\"
"
