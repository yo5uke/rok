#!/usr/bin/env bash
# V3b：Linux のバイナリが実行時に必要とするライブラリの不足を判定し、apt の実行時パッケージに対応づける。
#   1. 判定：展開済みのバイナリの .so を ldd で調べ、見つからない soname を集める（root もネットワークも不要）
#   2. 対応づけ：P3M の sysreqs（-dev パッケージ）→ apt-cache depends → 実行時のパッケージ → soname と名前で照合
#   3. 確認：提案した実行時パッケージだけを入れ、不足がなくなり sf が動くことを確かめる
# 前提：v01 の run.sh と v02 の run.sh を実行済みであること。ホストに docker があること。
# まっさらな ubuntu:24.04 に、R の実行時ライブラリ（v01 で特定）だけを入れた状態から始める。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out

SPIKES=$(cd .. && pwd)
R_HOME_DIR=$SPIKES/v01-linux-r-install/out/r/4.4.2/lib/R
CACHE=$SPIKES/v02-binary-extract/out/cache
R_RUNTIME=$(awk '{print $2}' "$SPIKES/v01-linux-r-install/out/soname-pkg.txt" | sort -u | tr '\n' ' ')

# P3M の sysreqs を、v02 で展開した全パッケージについてホストで取得しておく（コンテナには curl がない）
q=$(ls "$CACHE" | sed 's/^/pkgname=/' | paste -sd'&')
curl -fsS "https://packagemanager.posit.co/__api__/repos/cran/sysreqs?all=false&$q&distribution=ubuntu&release=24.04" |
  jq -r '.requirements[] | .name as $n | .requirements.packages[] | "\($n) \(.)"' > out/sysreqs.txt
echo "P3M sysreqs (-dev):"; sed 's/^/  /' out/sysreqs.txt

cat > out/inside.sh <<'EOF'
#!/bin/sh
# コンテナの中で、一般ユーザー（ホストと同じ uid）として実行する
set -eu
R_HOME_DIR=$1 CACHE=$2 OUT=$3

# 1. 判定：R の外から ldd を使うので、libR.so を見つけられるように R_HOME/lib を足す
start=$(date +%s%N)
for so in "$CACHE"/*/*/*/*/libs/*.so; do
  pkg=$(echo "${so#$CACHE/}" | cut -d/ -f1)
  LD_LIBRARY_PATH=$R_HOME_DIR/lib ldd "$so" | awk -v p="$pkg" '/not found/ {print p, $1}'
done | sort -u > "$OUT/missing.txt"
echo "check took $(( ($(date +%s%N) - start) / 1000000 )) ms for $(ls "$CACHE"/*/*/*/*/libs/*.so | wc -l) shared objects"
echo "missing:"; sed 's/^/  /' "$OUT/missing.txt"

# 2. 対応づけ：不足のある R パッケージの -dev パッケージから、apt-cache depends で実行時のパッケージを得る。
#    soname と実行時のパッケージ名を、英数字だけに正規化して照合する
#    （例：libgeos_c.so.1 → geosc1、libgeos-c1t64 → geosc1t64。前者が後者に含まれれば一致）
: > "$OUT/suggest.txt"
while read -r pkg so; do
  key=$(echo "$so" | sed 's/^lib//; s/\.so\././; s/[^A-Za-z0-9]//g')
  found=""
  for dev in $(awk -v p="$pkg" '$1 == p {print $2}' "$OUT/sysreqs.txt"); do
    for rt in $(apt-cache depends "$dev" | awk '/Depends: lib/ {print $2}' | grep -v -- '-dev$'); do
      norm=$(echo "$rt" | sed 's/^lib//; s/[^A-Za-z0-9]//g')
      case "$norm" in *"$key"*) found=$rt ;; esac
    done
  done
  echo "$pkg $so -> ${found:-（対応なし：-dev パッケージを案内する）}" | tee -a "$OUT/suggest.txt"
done < "$OUT/missing.txt"
EOF
chmod +x out/inside.sh

docker run --rm -v "$SPIKES":"$SPIKES" ubuntu:24.04 sh -c "
  apt-get update -qq >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends $R_RUNTIME >/dev/null 2>&1
  echo '## 1-2. 判定と対応づけ（一般ユーザー）'
  setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups $PWD/out/inside.sh $R_HOME_DIR $CACHE $PWD/out
  suggest=\$(awk '{print \$4}' $PWD/out/suggest.txt | grep -v '（' | sort -u | tr '\n' ' ')
  echo \"## 3. 提案した実行時パッケージだけを入れる：apt-get install \$suggest\"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \$suggest >/dev/null 2>&1
  setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups sh -c '
    for so in $CACHE/*/*/*/*/libs/*.so; do LD_LIBRARY_PATH=$R_HOME_DIR/lib ldd \$so; done | grep -c \"not found\" | sed \"s/^/remaining missing: /\"
    cd $SPIKES/v02-binary-extract
    $R_HOME_DIR/bin/Rscript -e \".libPaths(\\\"out/lib\\\"); suppressMessages(library(sf)); cat(\\\"sf loaded, buffer area:\\\", round(as.numeric(st_area(st_buffer(st_point(c(0, 0)), 1))), 3), \\\"\\\\n\\\")\"
  '
" 2>&1 | tee out/result.txt
