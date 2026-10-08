# V13（Windows 分）：rok を bench-rok.sh と同じ条件で測る（日付 2026-10-05、R 4.6、small・medium・gis）。
#   S4 cold：キャッシュとライブラリを消して `rok sync`
#   S3 warm：ライブラリだけを消して `rok sync`
#   S2 solve：索引をキャッシュ済みの状態で、同じ日付のまま全体を解き直す（`rok update --to <日付> --dry-run`）
#   S1 start：同期済みのプロジェクトで `Rscript -e "invisible(0)"`。基準は空の .Rprofile の空ディレクトリ
# 時間は Stopwatch（単調時計）。結果は out\results-rok-windows.csv、中央値を最後に表示する。
# 使い方：pwsh -File bench-rok-windows.ps1 -Rok <rok.exe> [-RHome <R のフォルダ>]
param(
  [Parameter(Mandatory)][string]$Rok,
  [string]$RHome = (Get-ChildItem "$env:ProgramFiles\R" -Directory | Sort-Object Name | Select-Object -Last 1).FullName,
  [string[]]$Projects = @("small", "medium", "gis"),
  [int]$NCold = 3, [int]$NWarm = 5, [int]$NSolve = 5, [int]$NStart = 10
)
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$Date = "2026-10-05"
$RootsOf = @{ small = @("fixest", "modelsummary"); medium = @("tidyverse"); gis = @("sf", "terra", "tmap") }
$out = Join-Path $PSScriptRoot "out\windows"
New-Item -ItemType Directory -Force "$out\proj", "$out\log" | Out-Null
$env:ROK_CACHE_DIR = "$out\cache"; $env:ROK_DATA_DIR = "$out\data"; $env:NO_COLOR = "1"; $env:ROK_BINARY = $Rok
$env:PATH = "$RHome\bin;$env:PATH"
$csv = "$out\results-rok-windows.csv"
"tool,project,scenario,run,seconds,status,npkgs" | Set-Content $csv

function Timed($tool, $project, $scenario, $run, [scriptblock]$body) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  & $body *> "$out\log\$tool-$project-$scenario-$run.log"
  $code = $LASTEXITCODE
  $secs = [math]::Round($sw.Elapsed.TotalSeconds, 3)
  $n = 0
  if ($tool -eq "rok") { $n = (Select-String -Path "$out\proj\rok-$project\rok.lock" -Pattern '^\[\[package\]\]' -ErrorAction SilentlyContinue).Count }
  "$tool,$project,$scenario,$run,$secs,$code,$n" | Add-Content $csv
  "{0,-5} {1,-7} {2,-6} #{3} {4,8}s status={5} pkgs={6}" -f $tool, $project, $scenario, $run, $secs, $code, $n
}

foreach ($p in $Projects) {
  $d = "$out\proj\rok-$p"
  Remove-Item -Recurse -Force $d, $env:ROK_CACHE_DIR -ErrorAction SilentlyContinue
  & $Rok init $d --r 4.6 --yes *> "$out\log\rok-$p-init.log"
  (Get-Content "$d\rok.toml") -replace '^snapshot = .*', "snapshot = `"$Date`"" | Set-Content "$d\rok.toml"
  # PowerShell の変数名は大文字と小文字を区別しないので、表とは別の名前にする
  $pkgs = $RootsOf[$p]
  Timed rok $p setup 1 { & $Rok --project $d add @pkgs --yes }
  foreach ($i in 1..$NCold) {
    Remove-Item -Recurse -Force $env:ROK_CACHE_DIR, "$d\.rok\library" -ErrorAction SilentlyContinue
    Timed rok $p cold $i { & $Rok --project $d sync --yes }
  }
  foreach ($i in 1..$NWarm) {
    Remove-Item -Recurse -Force "$d\.rok\library" -ErrorAction SilentlyContinue
    Timed rok $p warm $i { & $Rok --project $d sync --yes }
  }
  foreach ($i in 1..$NSolve) {
    Timed rok $p solve $i { & $Rok --project $d update --to $Date --dry-run }
  }
}

$baseline = "$out\proj\baseline"
New-Item -ItemType Directory -Force $baseline | Out-Null
Set-Content "$baseline\.Rprofile" ""
foreach ($p in $Projects) {
  $d = "$out\proj\rok-$p"
  & $Rok --project $d sync --yes *> "$out\log\rok-$p-start-resync.log"
  foreach ($i in 1..$NStart) { Timed none $p start $i { Push-Location $baseline; & Rscript -e "invisible(0)"; Pop-Location } }
  foreach ($i in 1..$NStart) { Timed rok $p start $i { Push-Location $d; & Rscript -e "invisible(0)"; Pop-Location } }
}

"## 中央値（秒）"
$rows = Import-Csv $csv | Where-Object scenario -ne "setup"
$rows | Group-Object tool, project, scenario | ForEach-Object {
  $v = @($_.Group | ForEach-Object { [double]$_.seconds } | Sort-Object)
  $m = if ($v.Count % 2) { $v[[int][math]::Floor($v.Count / 2)] } else { ($v[$v.Count / 2 - 1] + $v[$v.Count / 2]) / 2 }
  $bad = ($_.Group | Where-Object status -ne "0").Count
  "{0,-30} {1,8:N3}  n={2} 失敗={3} pkgs={4}" -f $_.Name, $m, $v.Count, $bad, $_.Group[0].npkgs
}
