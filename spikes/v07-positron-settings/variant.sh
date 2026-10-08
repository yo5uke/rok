#!/usr/bin/env bash
# V7：.vscode/settings.json を候補 N に入れ替える（0 は設定なし）。試験用のプロジェクトの中で使う。
#   ./v07-variant.sh 3
set -euo pipefail
cd "$(dirname "$0")"
n=${1:?候補の番号（0〜5）を指定してください}
if [ "$n" = 0 ]; then
  rm -f .vscode/settings.json
  echo "候補 0：設定なし"
else
  cp ".vscode/variants/$n.json" .vscode/settings.json
  echo "候補 $n：$(cat .vscode/settings.json)"
fi
echo "Positron で「Developer: Reload Window」を実行してから確かめてください。"
