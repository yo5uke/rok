#!/usr/bin/env bash
# V6・V1b（macOS 分）：Posit の portable な R ビルド（R-<版>-macos[-arm64].tar.gz）を、管理者権限なし・
# 画面なしで置けるか。複数の版の共存、P3M のバイナリ、ソースからのビルド、Gatekeeper（隔離の属性）を確かめる。
# GitHub Actions の macOS（.github/workflows/spikes.yml）で動かす。手元の Mac でも、そのフォルダで ./run.sh。
# R は out/R/<版> に置き、/Library/Frameworks には触れない。
set -euo pipefail
cd "$(dirname "$0")"
rm -rf out && mkdir -p out/R out/empty
OUT=$PWD/out
export R_LIBS_USER=$OUT/empty R_LIBS_SITE=$OUT/empty R_PROFILE_USER=$OUT/empty/none R_ENVIRON_USER=$OUT/empty/none
arch=$(uname -m)
suffix=$([ "$arch" = arm64 ] && echo "-macos-arm64" || echo "-macos")
base=https://cdn.posit.co/r/macos
VERSIONS=${VERSIONS:-"4.5.3 4.6.1"}
DATE=${DATE:-2026-10-01}
now() { python3 -c 'import time; print(time.monotonic())'; }
since() { python3 -c "import sys,time; print(round(time.monotonic() - float(sys.argv[1]), 1))" "$1"; }

echo "## 環境：macOS $(sw_vers -productVersion)、${arch}、Xcode のコマンドラインツール：$(xcode-select -p 2>/dev/null || echo なし)"

echo "## A. 提供されている版（versions.json の各版に HEAD）"
versions=$(curl -fsS https://cdn.posit.co/r/versions.json | python3 -c 'import json,sys; print(" ".join(json.load(sys.stdin)["r_versions"]))')
avail=(); missing=()
for v in $versions; do
  code=$(curl -sS -o /dev/null -I -w '%{http_code}' "$base/R-$v$suffix.tar.gz")
  if [ "$code" = 200 ]; then avail+=("$v"); else missing+=("$v"); fi
done
echo "あり（${#avail[@]}）：${avail[*]}"
echo "なし（${#missing[@]}）：${missing[*]}"

echo "## B. 取得と展開（管理者権限なし）"
for v in $VERSIONS; do
  t=$(now)
  curl -fsSL -o "out/R-$v.tar.gz" "$base/R-$v$suffix.tar.gz"
  dl=$(since "$t"); t=$(now)
  tar -xzf "out/R-$v.tar.gz" -C out/R
  echo "R ${v}：取得 $(du -m "out/R-$v.tar.gz" | cut -f1) MB を ${dl} 秒、展開 $(since "$t") 秒、展開後 $(du -sm "out/R/R-$v" | cut -f1) MB"
  echo "  隔離の属性（com.apple.quarantine）：$(xattr -r out/R/R-$v 2>/dev/null | grep -c quarantine || true) 件"
done
echo "  構成：$(ls out/R/R-${VERSIONS%% *} | tr '\n' ' ')"

for v in $VERSIONS; do
  rs=$OUT/R/R-$v/bin/Rscript
  echo "## C. R ${v}：起動、HTTPS、P3M のバイナリ、ソースからのビルド"
  "$rs" -e "cat(R.version.string, '\nR.home():', R.home(), '\n')"
  "$rs" ../v05-windows-r-install/check.R "$OUT/lib-$v" "$DATE" 2>&1 | grep -v -E '^\s*$|^Attaching|masked|%notin%'
  echo "  Gatekeeper（spctl）：$(spctl --assess --type execute "$OUT/R/R-$v/bin/exec/R" 2>&1 | head -1 || true)"
done

echo "## D. 共存：2つの版が互いの R_HOME を使わないか"
for v in $VERSIONS; do
  "$OUT/R/R-$v/bin/Rscript" -e "cat('$v', getRversion() == '$v', normalizePath(R.home()) == normalizePath('$OUT/R/R-$v'), '\n')"
done
