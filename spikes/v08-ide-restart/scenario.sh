#!/usr/bin/env bash
# 2-4 の確認：試験用のプロジェクトを、R の起動時に rok が対応する状態にする。
# 試験用のプロジェクトの中で使う。状態を作ったら、IDE で R を再起動して表示を確かめる。
#   ./v08-scenario.sh reset    同期済み（変化なし）に戻す
#   ./v08-scenario.sh light    R6 のリンクを外す（軽い同期：自動で直るはず）
#   ./v08-scenario.sh heavy    GitHub のパッケージを宣言する（ソースからのビルド：3択が出るはず）
#   ./v08-scenario.sh ask      [sync] on_startup = "ask"（軽い同期の前に尋ねるはず）
set -euo pipefail
cd "$(dirname "$0")"
data=$(Rscript --no-init-file -e 'cat(tools::R_user_dir("rok", "data"))')
rok=$data/bin/rok
reset() {
  sed -i '/^praise = /d; /^\[sync\]/d; /^on_startup = /d' rok.toml
  "$rok" sync --yes > /dev/null 2>&1
}
case ${1:?状態を指定してください（reset・light・heavy・ask）} in
  reset)
    reset
    echo "同期済みに戻しました。R を再起動すると「✔ rok: rok-ide-check (...)」の1行だけが出るはずです。"
    ;;
  light)
    reset
    rm -f .rok/library/R-*/*/R6
    echo "R6 のリンクを外しました。R を再起動すると自動で直り、「Restored in the library: R6」と出るはずです。"
    ;;
  heavy)
    reset
    sed -i '/^\[dependencies\]/a praise = { github = "gaborcsardi/praise" }' rok.toml
    echo "praise（GitHub、ソースからのビルドが要る）を宣言しました。R を再起動すると3択の質問が出るはずです。"
    ;;
  ask)
    reset
    printf '\n[sync]\non_startup = "ask"\n' >> rok.toml
    rm -f .rok/library/R-*/*/R6
    echo "on_startup = \"ask\" にして R6 のリンクを外しました。R を再起動すると、同期してよいかを尋ねるはずです。"
    ;;
  *)
    echo "reset・light・heavy・ask のどれかを指定してください。" >&2
    exit 1
    ;;
esac
