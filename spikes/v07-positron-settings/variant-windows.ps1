# V7（Windows）：.vscode\settings.json を候補 N に入れ替える（0 は設定なし）。試験用のプロジェクトの中で使う。
#   pwsh -File v07-variant.ps1 3
param([Parameter(Mandatory)][int]$N)
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location $PSScriptRoot
if ($N -eq 0) {
  Remove-Item -Force ".vscode\settings.json" -ErrorAction SilentlyContinue
  "候補 0：設定なし"
} else {
  Copy-Item ".vscode\variants\$N.json" ".vscode\settings.json" -Force
  "候補 ${N}：$(Get-Content .vscode\settings.json)"
}
"Positron で「Developer: Reload Window」を実行してから確かめてください。"
