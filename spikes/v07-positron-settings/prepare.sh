#!/usr/bin/env bash
# V7・V8 の準備：IDE で開く試験用のプロジェクトを作る（2-1）。
#
# 1. rok を release でビルドし、rok::setup() と同じ場所（R_user_dir("rok", "data")）に
#    バイナリと R パッケージを置く
# 2. R 4.5 を rok でユーザー領域に入れる（約 110 MB。入っていれば何もしない）
# 3. R 4.5 のプロジェクトを作り、R6 を入れる。システムの R（4.6）と区別できるよう、
#    プロジェクトの R はわざと 4.5 にする
# 4. .rok/bin/R（プロジェクトの R へのリンク）、Positron の設定の候補、確認用の R スクリプトを置く
#
# 使い方：./prepare.sh [プロジェクトを作る場所（既定：~/rok-ide-check）]
# 管理者権限は使わない。書き込む場所は、上の作業用のプロジェクトと R_user_dir("rok", ...) だけ。
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
proj=${1:-$HOME/rok-ide-check}

echo "## 1. rok のバイナリと R パッケージ"
(cd "$repo" && cargo build --release --quiet)
rok=$repo/target/release/rok
data=$(Rscript -e 'cat(tools::R_user_dir("rok", "data"))')
minor=4.5
mkdir -p "$data/bin"
cp "$rok" "$data/bin/rok.new" && mv "$data/bin/rok.new" "$data/bin/rok"
build=$(mktemp -d)
(cd "$build" && R CMD build --no-build-vignettes "$repo/rpkg" > /dev/null)
for m in 4.5 4.6; do
  mkdir -p "$data/library/R-$m"
done
echo "   binary: $data/bin/rok"

echo "## 2. R $minor"
"$data/bin/rok" r install "$minor" 2>&1 | tail -2
r45=$(ls -d "$data/r/$minor".* | sort -V | tail -1)
# The R package for each R that the checks use (setup() does this for the running R).
R CMD INSTALL --no-test-load -l "$data/library/R-4.6" "$build"/rok_*.tar.gz > /dev/null 2>&1
"$r45/bin/R" CMD INSTALL --no-test-load -l "$data/library/R-4.5" "$build"/rok_*.tar.gz > /dev/null 2>&1
rm -rf "$build"
echo "   R $minor: $r45"

echo "## 3. 試験用のプロジェクト：$proj"
if [ -e "$proj/rok.toml" ]; then
  echo "   すでにあるので作り直さない（作り直すときは、そのフォルダを消してから実行する）"
else
  "$data/bin/rok" init "$proj" --r "$minor" --name rok-ide-check --yes 2>&1 | head -2
  "$data/bin/rok" --project "$proj" add R6 --yes 2>&1 | tail -1
fi

echo "## 4. リンク・設定の候補・確認用スクリプト"
mkdir -p "$proj/.rok/bin" "$proj/.vscode/variants" "$proj/out"
ln -sfn "$r45/bin/R" "$proj/.rok/bin/R"
cp "$here/probe.R" "$proj/v07-probe.R"
cp "$here/variant.sh" "$proj/v07-variant.sh"
cp "$here/../v08-ide-restart/restart.R" "$proj/v08-restart.R"
cp "$here/../v08-ide-restart/after.R" "$proj/v08-after.R"
cp "$here/../v08-ide-restart/scenario.sh" "$proj/v08-scenario.sh"
chmod +x "$proj/v07-variant.sh" "$proj/v08-scenario.sh"
# Each start of R in the project is logged, to see whether a restart ran the startup hook.
if ! grep -q 'out/startup.log' "$proj/.Rprofile"; then
  printf '%s\n' 'cat(format(Sys.time(), "%H:%M:%OS3"), Sys.getpid(), getwd(), sep = "\t", file = "out/startup.log", append = TRUE); cat("\n", file = "out/startup.log", append = TRUE)' >> "$proj/.Rprofile"
fi

abs=$proj/.rok/bin/R
write() { printf '%s\n' "$2" > "$proj/.vscode/variants/$1.json"; }
write 1 "{ \"positron.r.customBinaries\": [\"$abs\"] }"
write 2 '{ "positron.r.customBinaries": [".rok/bin/R"] }'
write 3 '{ "positron.r.customBinaries": ["${workspaceFolder}/.rok/bin/R"] }'
write 4 "{ \"positron.r.customRootFolders\": [\"$data/r\"] }"
write 5 '{ "positron.r.customRootFolders": ["~/.local/share/R/rok/r"] }'
"$proj/v07-variant.sh" 0 > /dev/null

echo
echo "準備ができました。手順書（$here/README.md）の続きに進んでください。"
echo "IDE で開くフォルダ：$proj"
