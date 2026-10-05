#!/usr/bin/env bash
# V9：rok が出力する想定の renv.lock（Hash なし、P3M の日付つき URL、日付が混在するパッケージは日付ごとのリポジトリ）で、
# renv::restore() が動き、パッケージごとのリポジトリの指定と日付つき URL が守られるかを確かめる。
# renv のキャッシュや設定はすべて out/ に置き、ユーザー全体の環境には触れない。
# 前提：v01 の run.sh を実行済み（ユーザー領域の R 4.4.2 を使う）。
set -euo pipefail
cd "$(dirname "$0")"
rm -rf out && mkdir -p out/tools out/proj out/renv-root out/empty
OUT=$PWD/out
R_HOME_DIR=${R_HOME_DIR:-$PWD/../v01-linux-r-install/out/r/4.4.2}
RSCRIPT="$R_HOME_DIR/bin/Rscript"
# ユーザーのライブラリやサイトのライブラリを見ないようにする
export R_LIBS_USER=$OUT/empty R_LIBS_SITE=$OUT/empty R_PROFILE_USER=$OUT/empty/none R_ENVIRON_USER=$OUT/empty/none
export RENV_PATHS_ROOT=$OUT/renv-root

echo "## renv を道具用のライブラリに入れる（P3M 2026-10-01）"
"$RSCRIPT" -e "install.packages('renv', lib = '$OUT/tools', repos = 'https://packagemanager.posit.co/cran/__linux__/noble/2026-10-01', quiet = TRUE)
  cat('renv', format(packageVersion('renv', lib.loc = '$OUT/tools')), '/ R', format(getRversion()), '\n')"

echo "## renv.lock（Hash なし。2つの日付のリポジトリ、パッケージごとに Repository を指定）"
cat > out/proj/renv.lock <<'EOF'
{
  "R": {
    "Version": "4.4.2",
    "Repositories": [
      { "Name": "P3M-2024-06-03", "URL": "https://packagemanager.posit.co/cran/2024-06-03" },
      { "Name": "P3M-2023-06-01", "URL": "https://packagemanager.posit.co/cran/2023-06-01" }
    ]
  },
  "Packages": {
    "R6":         { "Package": "R6",         "Version": "2.5.1",  "Source": "Repository", "Repository": "P3M-2024-06-03" },
    "cli":        { "Package": "cli",        "Version": "3.6.2",  "Source": "Repository", "Repository": "P3M-2024-06-03" },
    "jsonlite":   { "Package": "jsonlite",   "Version": "1.8.8",  "Source": "Repository", "Repository": "P3M-2024-06-03" },
    "rlang":      { "Package": "rlang",      "Version": "1.1.3",  "Source": "Repository", "Repository": "P3M-2024-06-03" },
    "data.table": { "Package": "data.table", "Version": "1.14.8", "Source": "Repository", "Repository": "P3M-2023-06-01" },
    "glue":       { "Package": "glue",       "Version": "1.6.2",  "Source": "Repository", "Repository": "P3M-2023-06-01" }
  }
}
EOF
cat out/proj/renv.lock | grep -E '"(R6|data.table|glue)"' | sed 's/^ */  /'

# 入った版・バイナリかどうか（Built の日付が今日なら手元でビルドした）・取得元（renv が書く RemoteRepos）を表にする
report() {
  "$RSCRIPT" -e "
    lib <- '$1'
    want <- c(R6 = '2.5.1', cli = '3.6.2', jsonlite = '1.8.8', rlang = '1.1.3', data.table = '1.14.8', glue = '1.6.2')
    d <- lapply(names(want), function(p) read.dcf(file.path(lib, p, 'DESCRIPTION'), c('Version', 'Built', 'RemoteRepos'))[1, ])
    d <- as.data.frame(do.call(rbind, d), row.names = names(want))
    d\$want <- want
    d\$built_here <- grepl(format(Sys.Date()), d\$Built, fixed = TRUE)
    d\$RemoteRepos <- sub('https://packagemanager.posit.co/cran/', '', d\$RemoteRepos)
    print(d[, c('want', 'Version', 'built_here', 'RemoteRepos')])
    cat('all versions match:', all(d\$Version == want), '\n')
  "
}

restore() {
  rm -rf "$OUT/lib-$1" && mkdir -p "$OUT/lib-$1"
  (cd out/proj && cp "../$2" renv.lock && "$RSCRIPT" -e "
    .libPaths('$OUT/tools')
    renv::restore(library = '$OUT/lib-$1', prompt = FALSE)
  ") > "out/restore-$1.log" 2>&1 || { echo 'restore failed'; tail -20 "out/restore-$1.log"; exit 1; }
  echo "renv の警告：$(grep -ciE 'warning' "out/restore-$1.log") 件、installed binary：$(grep -c 'installed binary' "out/restore-$1.log") 件"
  report "$OUT/lib-$1"
}

cp out/proj/renv.lock out/with-repository.lock
echo "## A. renv::restore()（Repository を指定）"
restore a with-repository.lock

echo "## B. 比較：Repository 欄を消した場合（renv はリポジトリを順に探す）"
sed -E 's/, "Repository": "[^"]+"//' out/with-repository.lock > out/without-repository.lock
rm -rf "$RENV_PATHS_ROOT" && mkdir -p "$RENV_PATHS_ROOT"
restore b without-repository.lock
