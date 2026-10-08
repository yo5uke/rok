#!/usr/bin/env bash
# renv との往復テスト（要件定義書 第7章「renv.lock との互換」）。
# rok で作った環境を renv.lock に出力し、空の環境で renv::restore() して、
# 全パッケージの版（GitHub のパッケージはコミット）が rok.lock と一致するかを確かめる。
# 使い方：renv-roundtrip.sh <rok のバイナリ>。作業はすべて一時ディレクトリで行う。
set -euo pipefail

ROK=$(realpath "${1:?usage: renv-roundtrip.sh <rok binary>}")
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/empty" "$WORK/tools" "$WORK/lib" "$WORK/restore"

# rok と renv の置き場所を一時ディレクトリに向け、ユーザーやサイトのライブラリを見ないようにする
export ROK_CACHE_DIR=$WORK/cache ROK_DATA_DIR=$WORK/data NO_COLOR=1
export R_LIBS_USER=$WORK/empty R_LIBS_SITE=$WORK/empty
export R_PROFILE_USER=$WORK/empty/none R_ENVIRON_USER=$WORK/empty/none
export RENV_PATHS_ROOT=$WORK/renv-root

echo "::group::rok で環境を作る"
"$ROK" init "$WORK/proj" --yes
cd "$WORK/proj"
"$ROK" add R6 jsonlite cli --yes
# 古い版を範囲指定で選び、日付の混在を作る（出力では日付ごとのリポジトリを参照する）
"$ROK" add glue --version "< 1.7" --yes
"$ROK" add gaborcsardi/praise --yes
"$ROK" export renv --output "$WORK/restore/renv.lock" --yes
cat rok.lock
cat "$WORK/restore/renv.lock"
echo "::endgroup::"

# rok.lock から、名前・版・GitHub のコミットを取り出す
awk '
  /^\[\[package\]\]/ { if (name != "") print name "\t" version "\t" commit; name = version = commit = "" }
  /^name = /    { gsub(/"/, "", $3); name = $3 }
  /^version = / { gsub(/"/, "", $3); version = $3 }
  /commit = "/  { match($0, /commit = "[0-9a-f]+"/); commit = substr($0, RSTART + 10, RLENGTH - 11) }
  END { if (name != "") print name "\t" version "\t" commit }
' rok.lock > "$WORK/expected.tsv"
cat "$WORK/expected.tsv"

echo "::group::renv::restore()"
codename=$(. /etc/os-release && echo "$VERSION_CODENAME")
Rscript -e "install.packages('renv', lib = '$WORK/tools', quiet = TRUE,
  repos = 'https://packagemanager.posit.co/cran/__linux__/$codename/latest')"
(cd "$WORK/restore" && Rscript -e "
  .libPaths('$WORK/tools')
  cat('renv', format(packageVersion('renv')), '\n')
  renv::restore(library = '$WORK/lib', prompt = FALSE)
")
echo "::endgroup::"

Rscript -e "
  want <- read.delim('$WORK/expected.tsv', header = FALSE, colClasses = 'character',
                     col.names = c('name', 'version', 'commit'))
  got <- t(vapply(want\$name, function(p) {
    d <- packageDescription(p, lib.loc = '$WORK/lib')
    if (!is.list(d)) return(c(NA_character_, NA_character_))
    c(d\$Version, if (is.null(d\$RemoteSha)) '' else d\$RemoteSha)
  }, character(2)))
  want\$got_version <- got[, 1]
  want\$got_commit <- got[, 2]
  print(want, row.names = FALSE)
  bad <- is.na(want\$got_version) | want\$got_version != want\$version |
    (nzchar(want\$commit) & want\$got_commit != want\$commit)
  if (nrow(want) < 5 || any(bad)) stop('renv::restore() did not reproduce rok.lock')
  cat('All', nrow(want), 'packages match rok.lock.\n')
"
