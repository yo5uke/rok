# V11 の残り：OneDrive の同期フォルダの中にあるプロジェクトで、キャッシュ（OneDrive の外）へのジャンクションのライブラリが
# どう扱われるか。OneDrive がジャンクションやリンク先の属性を変えるか、R が読み込めるかを、時間をおいて確かめる。
# 再現：pwsh -File onedrive.ps1（run.ps1 を先に実行し、out\cache があること）。試験用のフォルダは最後に消す。
param(
  [string]$RHome = (Get-ChildItem "$env:ProgramFiles\R" -Directory | Sort-Object Name | Select-Object -Last 1).FullName,
  [int[]]$Waits = @(0, 30, 120)
)
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
if (-not $env:OneDrive -or -not (Test-Path $env:OneDrive)) { throw "OneDrive の同期フォルダがない" }
"OneDrive：$env:OneDrive、プロセス：$((Get-Process OneDrive -ErrorAction SilentlyContinue | Measure-Object).Count) 個"

$cache = Join-Path $PSScriptRoot "out\cache"
$proj = Join-Path $env:OneDrive "rok-v11-onedrive-test"
$lib = "$proj\.rok\library"
Remove-Item -Recurse -Force $proj -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $lib | Out-Null
Set-Content "$proj\analysis.R" "library(cli)"
$pkgs = "cli", "jsonlite", "glue"
foreach ($p in $pkgs) {
  $target = Get-ChildItem "$cache\$p" -Directory | Select-Object -First 1
  New-Item -ItemType Junction -Path "$lib\$p" -Target "$($target.FullName)\$p" | Out-Null
}
$probe = Get-ChildItem -Recurse -File "$cache\cli" | Select-Object -First 1

$rscript = Join-Path $RHome "bin\Rscript.exe"
$env:R_LIBS_USER = "$PSScriptRoot\out\empty"; $env:R_LIBS_SITE = "$PSScriptRoot\out\empty"
foreach ($w in $Waits) {
  Start-Sleep $w
  "## $w 秒待った後"
  foreach ($p in $pkgs) {
    $j = Get-Item "$lib\$p" -Force
    "  ジャンクション $p：LinkType=$($j.LinkType)、属性=$($j.Attributes)、DESCRIPTION を読める=$(Test-Path "$lib\$p\DESCRIPTION")"
  }
  "  ファイル analysis.R の属性=$((Get-Item "$proj\analysis.R").Attributes)"
  "  リンク先（キャッシュ）のファイルの属性=$((Get-Item $probe.FullName).Attributes)"
  $code = ".libPaths(c('$($lib -replace '\\','/')', .Library)); cat(all(vapply(c('cli','jsonlite','glue'), requireNamespace, logical(1), quietly = TRUE)))"
  "  R で3つとも読み込めたか：$(& $rscript -e $code)"
}

"## 後片付け（ジャンクションは再帰なしで消す）"
foreach ($p in $pkgs) {
  try { [IO.Directory]::Delete("$lib\$p"); "  $p：そのまま消せた" }
  catch {
    # OneDrive はジャンクションに ReadOnly を付ける。外してから消す
    $j = Get-Item "$lib\$p" -Force
    $j.Attributes = $j.Attributes -band -bnot [IO.FileAttributes]::ReadOnly
    [IO.Directory]::Delete("$lib\$p")
    "  $p：そのままでは消せず（$($_.Exception.InnerException.Message.Trim())）、ReadOnly を外して消せた"
  }
}
Remove-Item -Recurse -Force $proj
"消した。キャッシュの実体は残る：$(Test-Path $probe.FullName)"
