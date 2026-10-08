# 2-4 の確認（Windows）：試験用のプロジェクトを、R の起動時に rok が対応する状態にする。scenario.sh の Windows 版。
# 試験用のプロジェクトの中で使う。状態を作ったら、IDE で R を再起動して表示を確かめる。
#   pwsh -File v08-scenario.ps1 reset    同期済み（変化なし）に戻す
#   pwsh -File v08-scenario.ps1 light    R6 のリンクを外す（軽い同期：自動で直るはず）
#   pwsh -File v08-scenario.ps1 heavy    GitHub のパッケージを宣言する（ソースからのビルド：3択が出るはず）
#   pwsh -File v08-scenario.ps1 ask      [sync] on_startup = "ask"（軽い同期の前に尋ねるはず）
param([Parameter(Mandatory)][ValidateSet("reset", "light", "heavy", "ask")][string]$State)
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location $PSScriptRoot
$data = Rscript --no-init-file -e "cat(tools::R_user_dir('rok', 'data'))"
$rok = "$data\bin\rok.exe"
function Reset-Project {
  (Get-Content rok.toml) | Where-Object { $_ -notmatch '^praise = |^\[sync\]|^on_startup = ' } | Set-Content rok.toml
  & $rok sync --yes *> $null
}
function Remove-R6 {
  # ジャンクションは再帰せずに消す（OneDrive の中なら ReadOnly を外してから）
  foreach ($j in Get-ChildItem ".rok\library\R-*\*\R6" -Force -ErrorAction SilentlyContinue) {
    $j.Attributes = $j.Attributes -band -bnot [IO.FileAttributes]::ReadOnly
    [IO.Directory]::Delete($j.FullName)
  }
}
switch ($State) {
  "reset" {
    Reset-Project
    "同期済みに戻しました。R を再起動すると「✔ rok: rok-ide-check (...)」の1行だけが出るはずです。"
  }
  "light" {
    Reset-Project; Remove-R6
    "R6 のリンクを外しました。R を再起動すると自動で直り、「Restored in the library: R6」と出るはずです。"
  }
  "heavy" {
    Reset-Project
    (Get-Content rok.toml) | ForEach-Object { $_; if ($_ -eq "[dependencies]") { 'praise = { github = "gaborcsardi/praise" }' } } | Set-Content rok.toml
    "praise（GitHub、ソースからのビルドが要る）を宣言しました。R を再起動すると3択の質問が出るはずです。"
  }
  "ask" {
    Reset-Project
    Add-Content rok.toml "`n[sync]`non_startup = `"ask`""
    Remove-R6
    "on_startup = `"ask`" にして R6 のリンクを外しました。R を再起動すると、同期してよいかを尋ねるはずです。"
  }
}
