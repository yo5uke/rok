#!/bin/sh
# V3c：ソースからのビルドに必要なビルドツールと -dev パッケージの不足を判定し、apt のコマンドを組み立てる。
# rok に組み込む判定の原型。root もネットワークも使わない（dpkg の記録と、手元のソースの tar.gz だけを見る）。
# 使い方：detect.sh <R の bin/R> <sysreqs.txt（"パッケージ -devパッケージ" の行）> <ソースの tar.gz>...
set -eu
R=$1 SYSREQS=$2; shift 2

need_c=0 need_cxx=0 need_fc=0 pkgs=""
for tgz in "$@"; do
  pkg=$(tar -tzf "$tgz" | head -1 | cut -d/ -f1)
  pkgs="$pkgs $pkg"
  files=$(tar -tzf "$tgz" | grep "^$pkg/src/" || true)
  # 言語の判定：src/ の拡張子と、LinkingTo（Rcpp などは C++ を意味する）
  echo "$files" | grep -qE '\.c$' && need_c=1
  echo "$files" | grep -qE '\.(cpp|cc|cxx)$' && need_cxx=1
  echo "$files" | grep -qiE '\.(f|f90|f95)$' && need_fc=1
  tar -xOzf "$tgz" "$pkg/DESCRIPTION" | grep -qE '^LinkingTo:.*(Rcpp|cpp11|BH)' && need_cxx=1
  # src/ があれば、C のリンクと make は必ず要る
  [ -n "$files" ] && need_c=1
done

missing_tools="" missing_dev=""
# R が使うコンパイラは Makeconf から読む（R CMD config は内部で make を使うので、make がないと使えない）
MAKECONF=$("$R" RHOME)/etc/Makeconf
tool() { # 言語の要否、Makeconf の変数名、apt のパッケージ名
  [ "$1" = 1 ] || return 0
  cmd=$(awk -v k="$2" '$1 == k && $2 == "=" {print $3; exit}' "$MAKECONF")
  # MAKE は Makeconf ではなく etc/Renviron で決まる（MAKE=${MAKE-'make'}）。環境変数がなければ make
  [ "$2" = MAKE ] && cmd=${MAKE:-make}
  command -v "$cmd" >/dev/null 2>&1 || missing_tools="$missing_tools $3"
}
tool "$need_c" CC gcc
tool "$need_cxx" CXX g++
tool "$need_fc" FC gfortran
tool 1 MAKE make

for pkg in $pkgs; do
  for dev in $(awk -v p="$pkg" '$1 == p {print $2}' "$SYSREQS"); do
    status=$(dpkg-query -W -f='${db:Status-Abbrev}' "$dev" 2>/dev/null || true)
    case $status in ii*) ;; *) missing_dev="$missing_dev $dev" ;; esac
  done
done

echo "needs: C=$need_c C++=$need_cxx Fortran=$need_fc"
echo "missing build tools:${missing_tools:- none}"
echo "missing -dev packages:${missing_dev:- none}"
all="$missing_tools$missing_dev"
if [ -n "$all" ]; then
  # apt のリストがない環境では update を添える（V3b）
  if [ -z "$(ls /var/lib/apt/lists 2>/dev/null | grep -v -e lock -e partial)" ]; then pre="sudo apt-get update && "; else pre=""; fi
  echo "suggest: ${pre}sudo apt-get install -y$all"
  echo "$all" > "${DETECT_OUT:-/dev/null}"
fi
