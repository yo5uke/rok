#!/usr/bin/env bash
# V2（macOS 分）：P3M の macOS のバイナリ（tgz）を、R を使わずに展開するだけで library() が通るかを確かめる。
# あわせて、索引の Hash（MD5）との照合と、R CMD INSTALL で入れた場合との比較を行う。
# R は ../v06-macos-r-install/run.sh が置いた portable な R を使う（RHOME で変えられる）。
# GitHub Actions の macOS（.github/workflows/spikes.yml）で動かす。
set -euo pipefail
cd "$(dirname "$0")"
OUT=$PWD/out/macos
rm -rf "$OUT" && mkdir -p "$OUT/tgz" "$OUT/lib" "$OUT/lib-r" "$OUT/empty"
RHOME=${RHOME:-$(cd ../v06-macos-r-install/out/R && ls -d R-4.6.* | tail -1 | sed "s|^|$PWD/|")}
DATE=${DATE:-2026-10-01}
export R_LIBS_USER=$OUT/empty R_LIBS_SITE=$OUT/empty R_PROFILE_USER=$OUT/empty/none R_ENVIRON_USER=$OUT/empty/none
rs=$RHOME/bin/Rscript
minor=$("$rs" windows-minor.R)
macos=$(curl -fsS https://packagemanager.posit.co/__api__/status |
  python3 -c "import json,sys; d=json.load(sys.stdin)['macos_urls']; a='arm64' if '$(uname -m)'=='arm64' else 'x86_64'; print((d.get('$minor') or d['default'])[a])")
repo=https://packagemanager.posit.co/cran/$DATE/bin/macosx/$macos/contrib/$minor
echo "## R: $RHOME（R $minor）、P3M: $repo"

"$rs" windows-deps.R "$repo" "$OUT/packages.csv" sf data.table R6 cli
n=$(($(wc -l < "$OUT/packages.csv") - 1))
echo "## 対象：$n パッケージ"

t=$(python3 -c 'import time; print(time.monotonic())')
types=""; md5ok=0
while IFS=, read -r pkg ver hash; do
  pkg=${pkg//\"/}; ver=${ver//\"/}; hash=${hash//\"/}
  [ "$pkg" = package ] && continue
  file="${pkg}_${ver}.tgz"
  type=$(curl -sS -o /dev/null -D - "$repo/$file" | tr -d '\r' | awk -F': ' 'tolower($1)=="x-package-type"{print $2}')
  types="$types $type"
  curl -fsSL -o "$OUT/tgz/$file" "$repo/$file"
  [ "$(md5 -q "$OUT/tgz/$file")" = "$hash" ] && md5ok=$((md5ok + 1)) || echo "MD5 mismatch: $file"
done < "$OUT/packages.csv"
echo "## 取得：$(du -sm "$OUT/tgz" | cut -f1) MB を $(python3 -c "import time; print(round(time.monotonic() - $t, 1))") 秒（逐次）。種類：$(echo $types | tr ' ' '\n' | sort | uniq -c | tr '\n' ' ')"
echo "## MD5（索引の Hash）：$md5ok / $n が一致"

t=$(python3 -c 'import time; print(time.monotonic())')
for f in "$OUT"/tgz/*.tgz; do tar -xzf "$f" -C "$OUT/lib"; done
echo "## 展開：$(python3 -c "import time; print(round(time.monotonic() - $t, 2))") 秒、展開後 $(du -sm "$OUT/lib" | cut -f1) MB"
echo "  隔離の属性：$(xattr -r "$OUT/lib" 2>/dev/null | grep -c quarantine || true) 件"

echo "## 読み込みと動作（R に展開先のライブラリだけを見せる）"
"$rs" windows-check.R "$OUT/lib" $(tail -n +2 "$OUT/packages.csv" | cut -d, -f1 | tr -d '"') 2>&1 | grep -v -E '^\s*$|^Attaching|masked|%notin%'

echo "## R CMD INSTALL との比較（R6、data.table、sf）"
for name in R6 data.table sf; do
  ver=$(grep "^\"$name\"," "$OUT/packages.csv" | cut -d, -f2 | tr -d '"')
  "$RHOME/bin/R" CMD INSTALL --library="$OUT/lib-r" "$OUT/tgz/${name}_${ver}.tgz" > "$OUT/install-$name.log" 2>&1
  if diff -r "$OUT/lib/$name" "$OUT/lib-r/$name" > "$OUT/diff-$name.txt"; then
    echo "$name：同一（$(find "$OUT/lib/$name" -type f | wc -l | tr -d ' ') ファイル）"
  else
    echo "$name：違いあり"; head -5 "$OUT/diff-$name.txt"
  fi
done
