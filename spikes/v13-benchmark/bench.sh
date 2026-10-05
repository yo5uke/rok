#!/usr/bin/env bash
# V13（Linux 分）：renv・pak・rv・uvr の速度を、同じ R・同じ P3M の日付・同じプロジェクトで測る。
#
# 場面（要件定義書 第9章）
#   S4 cold  キャッシュなし：ツールのキャッシュとプロジェクトのライブラリを消し、ロックから入れる（N_COLD 回）
#   S3 warm  キャッシュあり：プロジェクトのライブラリだけを消し、ロックから入れる（N_WARM 回）
#   S2 solve 依存の解決：索引をキャッシュ済みの状態で、ロックを作り直す（renv には解決だけの操作がないので除く）
#   S1 start 起動時のチェック：同期済みのプロジェクトで `Rscript -e 'invisible(0)'` にかかる時間（基準との差は集計で出す）
# 結果は out/results.csv に追記する。集計は summarize.py。
#
# 前提：v01 の run.sh を実行済み（ユーザー領域の R 4.6.1 を使う）。rv・uvr のバイナリは out/bin に取得する。
# ツールのキャッシュや設定はすべて out/ に置き、ユーザー全体の環境には触れない。
set -uo pipefail
cd "$(dirname "$0")"
mkdir -p out/bin out/tools out/empty
OUT=$PWD/out

DATE=${DATE:-2026-10-05}           # P3M の日付（uvr は日付を固定できず、cran/latest を使う）
TOOLS=${TOOLS:-"renv pak rv uvr"}
PROJECTS=${PROJECTS:-"small medium gis"}
N_COLD=${N_COLD:-3} N_WARM=${N_WARM:-5} N_SOLVE=${N_SOLVE:-5} N_START=${N_START:-10}
PHASES=${PHASES:-"main start"}   # main：準備・S4・S3・S2、start：S1
RV_VERSION=v0.23.1 UVR_VERSION=v0.4.6
declare -A ROOTS=([tiny]="R6 cli" [small]="fixest modelsummary" [medium]="tidyverse" [gis]="sf terra tmap")  # tiny は動作確認用

R_HOME_DIR=${R_HOME_DIR:-$PWD/../v01-linux-r-install/out/r/4.6.1}
export PATH=$R_HOME_DIR/bin:$OUT/bin:$PATH
export R_LIBS_USER=$OUT/empty R_LIBS_SITE=$OUT/empty R_PROFILE_USER=$OUT/empty/none R_ENVIRON_USER=$OUT/empty/none
# ツールがホームに書く分（uvr の ~/.uvr/packages など）も out/ に閉じ込める
export HOME=$OUT/home XDG_CACHE_HOME=$OUT/cache/xdg
mkdir -p "$HOME"
export RENV_PATHS_ROOT=$OUT/cache/renv R_USER_CACHE_DIR=$OUT/cache/pak RV_CACHE_DIR=$OUT/cache/rv UVR_CACHE_DIR=$OUT/cache/uvr
REPO=https://packagemanager.posit.co/cran/$DATE
REPO_BIN=https://packagemanager.posit.co/cran/__linux__/noble/$DATE

# ---- 道具の用意（測らない） ----
[ -x out/bin/rv ] || curl -fsSL "https://github.com/A2-ai/rv/releases/download/$RV_VERSION/rv-$RV_VERSION-x86_64-unknown-linux-gnu.tar.gz" | tar -xz -C out/bin
[ -x out/bin/uvr ] || curl -fsSL "https://github.com/nbafrank/uvr/releases/download/$UVR_VERSION/uvr-x86_64-unknown-linux-gnu.tar.gz" | tar -xz -C out/bin
[ -d out/tools/pak ] || Rscript -e "install.packages(c('renv', 'pak'), lib = '$OUT/tools', repos = '$REPO_BIN', quiet = TRUE)"
{
  echo "date=$DATE"
  echo "R=$("$R_HOME_DIR/bin/Rscript" -e 'cat(format(getRversion()))')"
  echo "renv=$(Rscript -e "cat(format(packageVersion('renv', lib.loc = '$OUT/tools')))")"
  echo "pak=$(Rscript -e "cat(format(packageVersion('pak', lib.loc = '$OUT/tools')))")"
  echo "rv=$(rv --version)"
  echo "uvr=$(uvr --version)"
  echo "host=$(nproc) cores, $(uname -r)"
} > out/versions.txt
cat out/versions.txt

# ---- 計測の道具 ----
record() { # tool project scenario run seconds status npkgs
  echo "$1,$2,$3,$4,$5,$6,$7" >> out/results.csv
}
# renv はサンドボックスを書き込み禁止にするので、消す前に権限を戻す
wipe() { [ -e "$1" ] && chmod -R u+w "$1"; rm -rf "${1:?}"; }
[ -f out/results.csv ] || echo "tool,project,scenario,run,seconds,status,npkgs" > out/results.csv
count_pkgs() { find -L "$1" -mindepth 2 -maxdepth 2 -name DESCRIPTION 2>/dev/null | wc -l; }
rlist() { echo "$1" | sed "s/[^ ][^ ]*/'&'/g; s/ /, /g"; }   # "a b" → 'a', 'b'

# ---- ツールごとの操作 ----
# dir <tool> <project>：プロジェクトのディレクトリ
dir_of() { echo "$OUT/proj/$1-$2"; }
# lib <tool> <project>：プロジェクトのライブラリ
lib_of() {
  local d; d=$(dir_of "$1" "$2")
  case $1 in
    renv|pak) echo "$d/lib" ;;
    rv) echo "$d/rv/library/4.6/x86_64/noble" ;;
    uvr) echo "$d/.uvr/library" ;;
  esac
}
# ツールのキャッシュは out/cache/<ツール> に置いている（環境変数を参照）。uvr は ~/.uvr も使う
clear_cache() {
  wipe "${OUT:?}/cache/${1:?}"
  [ "$1" = uvr ] && rm -rf "${OUT:?}/home/.uvr"   # uvr はパッケージの実体を ~/.uvr/packages に置く
  return 0
}

# setup：プロジェクトを作り、ロックを作る（キャッシュも温まる）
setup() {
  local t=$1 p=$2 plain=$3 d roots; d=$(dir_of "$t" "$p"); roots=$(rlist "$plain")
  rm -rf "${d:?}" && mkdir -p "$d"
  case $t in
    renv) Rscript -e "
      .libPaths('$OUT/tools'); options(repos = c(CRAN = '$REPO'))
      dir.create('$d/lib')
      renv::install(c($roots), library = '$d/lib', project = '$d', prompt = FALSE)
      # 推奨パッケージ（lattice など）は R 本体のライブラリにあり、library = の指定外なので事前チェックに落ちる。force で通す
      renv::snapshot(project = '$d', library = '$d/lib', packages = c($roots), prompt = FALSE, force = TRUE)" ;;
    pak) Rscript -e "
      .libPaths('$OUT/tools'); options(repos = c(CRAN = '$REPO_BIN'))
      pak::lockfile_create(c($roots), lockfile = '$d/pkg.lock', lib = '$OUT/empty')
      pak::lockfile_install('$d/pkg.lock', lib = '$d/lib')" ;;
    rv) printf '[project]\nname = "%s"\nr_version = "4.6"\nrepositories = [\n    { alias = "P3M", url = "%s" },\n]\ndependencies = [%s]\n' \
          "$p" "$REPO" "$(echo "$plain" | sed 's/[^ ][^ ]*/"&"/g; s/ /, /g')" > "$d/rproject.toml"
        (cd "$d" && rv sync && rv activate) ;;   # rv activate は .Rprofile（起動フック）を作る
    uvr) (cd "$d" && uvr init --here && uvr add $plain) ;;
  esac
}

# install：ロックからプロジェクトのライブラリに入れる（測る対象）
install() {
  local t=$1 p=$2 d; d=$(dir_of "$t" "$p")
  case $t in
    renv) mkdir -p "$d/lib" && Rscript -e "
      .libPaths('$OUT/tools'); options(repos = c(CRAN = '$REPO'))
      renv::restore(project = '$d', library = '$d/lib', prompt = FALSE)" ;;
    pak) Rscript -e "
      .libPaths('$OUT/tools'); options(repos = c(CRAN = '$REPO_BIN'))
      pak::lockfile_install('$d/pkg.lock', lib = '$d/lib')" ;;
    rv) (cd "$d" && rv sync) ;;
    uvr) (cd "$d" && uvr sync) ;;
  esac
}

# solve：依存の解決だけを行う（索引はキャッシュ済み）
solve() {
  local t=$1 p=$2 plain=$3 d roots; d=$(dir_of "$t" "$p"); roots=$(rlist "$plain")
  case $t in
    pak) Rscript -e "
      .libPaths('$OUT/tools'); options(repos = c(CRAN = '$REPO_BIN'))
      pak::lockfile_create(c($roots), lockfile = '$d/solve.lock', lib = '$OUT/empty')" ;;
    rv) cp "$d/rv.lock" "$d/rv.lock.keep" && rm -f "$d/rv.lock" && (cd "$d" && rv plan); local s=$?; mv "$d/rv.lock.keep" "$d/rv.lock"; return $s ;;
    uvr) (cd "$d" && uvr lock) ;;
    *) return 99 ;;
  esac
}

timed() { # tool project scenario run -- command...（時間は単調時計で測る。mono.py を参照）
  local t=$1 p=$2 s=$3 r=$4; shift 5
  local secs status
  read -r secs status < <(python3 "$OUT/../mono.py" "$OUT/log/$t-$p-$s-$r.log" "$@")
  local n; n=$(count_pkgs "$(lib_of "$t" "$p")")
  record "$t" "$p" "$s" "$r" "$secs" "$status" "$n"
  printf '%-5s %-7s %-6s #%s %8ss status=%s pkgs=%s\n' "$t" "$p" "$s" "$r" "$secs" "$status" "$n"
}

export OUT REPO REPO_BIN
export -f dir_of lib_of rlist setup install solve
mkdir -p out/log
[[ " $PHASES " == *" main "* ]] && for p in $PROJECTS; do
  for t in $TOOLS; do
    lib=$(lib_of "$t" "$p")
    # ロックを作る（キャッシュの状態をそろえるため、まず消す）
    clear_cache "$t"
    timed "$t" "$p" setup 1 -- setup "$t" "$p" "${ROOTS[$p]}"
    for i in $(seq "$N_COLD"); do
      clear_cache "$t"; rm -rf "${lib:?}"
      timed "$t" "$p" cold "$i" -- install "$t" "$p"
    done
    for i in $(seq "$N_WARM"); do
      rm -rf "${lib:?}"
      timed "$t" "$p" warm "$i" -- install "$t" "$p"
    done
    if [ "$t" != renv ]; then
      for i in $(seq "$N_SOLVE"); do timed "$t" "$p" solve "$i" -- solve "$t" "$p" "${ROOTS[$p]}"; done
    fi
  done
done

# S1：同期済み（変化なし）のプロジェクトで R を起動する時間。基準は、何も読み込まない空のディレクトリ。
# R_PROFILE_USER を外すと、R は作業ディレクトリの .Rprofile を読む（HOME は out/home なので、ユーザーの .Rprofile は読まない）。
start_r() { (cd "$1" && env -u R_PROFILE_USER Rscript -e 'invisible(0)'); }
# renv は、計測用のプロジェクトを有効にしていないので、S1 用に有効にしたプロジェクトを別に作る。
# restore の後に snapshot して renv 自身もロックに入れ、「変化なし」の状態にする。pak には起動フックがないので測らない。
prepare_renv_start() {
  local p=$1 d="$OUT/proj/renv-start-$1"
  rm -rf "${d:?}" && mkdir -p "$d" && cp "$(dir_of renv "$p")/renv.lock" "$d/"
  Rscript -e ".libPaths('$OUT/tools'); renv::activate(project = '$d')" &&
    (cd "$d" && env -u R_PROFILE_USER Rscript -e "options(repos = c(CRAN = '$REPO')); renv::restore(prompt = FALSE); renv::snapshot(prompt = FALSE, force = TRUE)")
}
export -f start_r
mkdir -p out/proj/baseline && : > out/proj/baseline/.Rprofile
[[ " $PHASES " == *" start "* ]] && for p in $PROJECTS; do
  [[ " $TOOLS " == *" renv "* ]] && prepare_renv_start "$p" > "out/log/renv-$p-start-prepare.log" 2>&1
  for i in $(seq "$N_START"); do timed none "$p" start "$i" -- start_r "$OUT/proj/baseline"; done
  for t in renv rv uvr; do
    [[ " $TOOLS " == *" $t "* ]] || continue
    d=$(dir_of "$t" "$p"); [ "$t" = renv ] && d="$OUT/proj/renv-start-$p"
    # 後のプロジェクトの cold でキャッシュを消すと、キャッシュへのリンクで組んだライブラリ（uvr）は壊れる。
    # uvr sync は切れたリンクを直せない（"Failed to move staged package"）ので、ライブラリを消してから入れ直し、
    # 「変化なし」の状態で測る
    if [ "$t" != renv ]; then lib=$(lib_of "$t" "$p"); rm -rf "${lib:?}"; install "$t" "$p" > "out/log/$t-$p-start-resync.log" 2>&1; fi
    for i in $(seq "$N_START"); do timed "$t" "$p" start "$i" -- start_r "$d"; done
  done
done
