#!/usr/bin/env bash
# V13（追記、1-9）：rok を bench.sh と同じ条件で測る。
#   日付 2026-10-05、R 4.6.1、時間は単調時計（mono.py）、small・medium・gis。
#   S4 cold：キャッシュ（ROK_CACHE_DIR）とライブラリを消して `rok sync`
#   S3 warm：ライブラリだけを消して `rok sync`
#   S2 solve：索引をキャッシュ済みの状態で、同じ日付のまま全体を解き直す（`rok update --to <日付> --dry-run`）
#   S1 start：同期済みのプロジェクトで `Rscript -e 'invisible(0)'`（activate.R が `rok activate` で
#             変化の有無を確かめる）。基準（none）は空の .Rprofile の空ディレクトリ
# 結果は out/results-rok.csv に書く。集計は `python3 summarize.py out/results-rok.csv`。
# 使い方：ROK=<rok のバイナリ> ./bench-rok.sh（既定は ../../target/release/rok）
set -euo pipefail
cd "$(dirname "$0")"
OUT=$PWD/out
ROK=${ROK:-$(cd ../.. && pwd)/target/release/rok}
DATE=2026-10-05
N_COLD=${N_COLD:-3} N_WARM=${N_WARM:-5} N_SOLVE=${N_SOLVE:-5} N_START=${N_START:-10}
PROJECTS=${PROJECTS:-"small medium gis"}
declare -A ROOTS=([small]="fixest modelsummary" [medium]="tidyverse" [gis]="sf terra tmap")
# ROK_BINARY: activate.R finds the binary there (instead of the place setup() puts it).
export ROK_CACHE_DIR=$OUT/rok/cache ROK_DATA_DIR=$OUT/rok/data NO_COLOR=1 HOME=$OUT/home ROK_BINARY=$ROK
CSV=$OUT/results-rok.csv
mkdir -p "$OUT/log" "$OUT/proj"
[ -f "$CSV" ] || echo "tool,project,scenario,run,seconds,status,npkgs" > "$CSV"

proj() { echo "$OUT/proj/rok-$1"; }
count() { grep -c '^\[\[package\]\]' "$(proj "$1")/rok.lock" 2>/dev/null || true; }
timed() { # tool project scenario run -- command...
  local t=$1 p=$2 s=$3 r=$4; shift 5
  local secs status
  read -r secs status < <(python3 "$OUT/../mono.py" "$OUT/log/$t-$p-$s-$r.log" "$@")
  local n=0; [ "$t" = rok ] && n=$(count "$p")
  echo "$t,$p,$s,$r,$secs,$status,$n" >> "$CSV"
  printf '%-5s %-7s %-6s #%s %8ss status=%s pkgs=%s\n' "$t" "$p" "$s" "$r" "$secs" "$status" "$n"
}

for p in $PROJECTS; do
  d=$(proj "$p")
  rm -rf "${d:?}" "${ROK_CACHE_DIR:?}"
  "$ROK" init "$d" --r 4.6 --yes > "$OUT/log/rok-$p-init.log" 2>&1
  sed -i "s/^snapshot = .*/snapshot = \"$DATE\"/" "$d/rok.toml"
  # shellcheck disable=SC2086
  timed rok "$p" setup 1 -- "$ROK" --project "$d" add ${ROOTS[$p]} --yes
  for i in $(seq "$N_COLD"); do
    rm -rf "${ROK_CACHE_DIR:?}" "${d:?}/.rok/library"
    timed rok "$p" cold "$i" -- "$ROK" --project "$d" sync --yes
  done
  for i in $(seq "$N_WARM"); do
    rm -rf "${d:?}/.rok/library"
    timed rok "$p" warm "$i" -- "$ROK" --project "$d" sync --yes
  done
  for i in $(seq "$N_SOLVE"); do
    timed rok "$p" solve "$i" -- "$ROK" --project "$d" update --to "$DATE" --dry-run
  done
done

start_r() { (cd "$1" && env -u R_PROFILE_USER Rscript -e 'invisible(0)'); }
export -f start_r
mkdir -p "$OUT/proj/baseline" && : > "$OUT/proj/baseline/.Rprofile"
for p in $PROJECTS; do
  # 後のプロジェクトの cold でキャッシュを消したので、入れ直して「変化なし」にする
  "$ROK" --project "$(proj "$p")" sync --yes > "$OUT/log/rok-$p-start-resync.log" 2>&1
  for i in $(seq "$N_START"); do timed none "$p" start "$i" -- start_r "$OUT/proj/baseline"; done
  for i in $(seq "$N_START"); do timed rok "$p" start "$i" -- start_r "$(proj "$p")"; done
done
