# V5・V1b（Windows 分）：Posit の portable な R ビルドを、管理者権限なし・画面なし・レジストリなしで置けるか。
# 複数の版の共存、P3M のバイナリパッケージ、Rtools によるソースからのビルドも確かめる。
# 再現：pwsh -File run.ps1 [-Versions 4.5.3,4.6.1] [-Date 2026-10-01]
# R は out\Programs\R\R-<版> に置く（実際の %LOCALAPPDATA%\Programs\R には触れない）。
param(
  [string[]]$Versions = @("4.5.3", "4.6.1"),
  [string]$Date = "2026-10-01"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -AssemblyName System.IO.Compression.FileSystem

$out = Join-Path $PSScriptRoot "out"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$out\zip", "$out\Programs\R", "$out\empty" | Out-Null
$env:R_LIBS_USER = "$out\empty"; $env:R_LIBS_SITE = "$out\empty"
$env:R_PROFILE_USER = "$out\empty\none"; $env:R_ENVIRON_USER = "$out\empty\none"
$base = "https://cdn.posit.co/r/windows"

"## A. 提供されている版（versions.json の各版に HEAD）"
$client = [Net.Http.HttpClient]::new()
$all = (Invoke-RestMethod "https://cdn.posit.co/r/versions.json").r_versions
$avail = @(); $missing = @()
foreach ($v in $all) {
  $req = [Net.Http.HttpRequestMessage]::new([Net.Http.HttpMethod]::Head, "$base/R-$v-windows.zip")
  if ($client.SendAsync($req).Result.IsSuccessStatusCode) { $avail += $v } else { $missing += $v }
}
"あり（$($avail.Count)）：$($avail -join ' ')"
"なし（$($missing.Count)）：$($missing -join ' ')"

# レジストリのうち、R のインストーラーが書く場所
function Get-RRegistry {
  foreach ($k in "HKCU:\Software\R-core", "HKLM:\Software\R-core") {
    if (Test-Path $k) {
      Get-ChildItem -Recurse $k | ForEach-Object { $key = $_; $key.Name + " " + (($key.GetValueNames() | ForEach-Object { "$_=" + $key.GetValue($_) }) -join ",") }
    }
  }
  foreach ($k in "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall") {
    if (Test-Path $k) { Get-ChildItem $k | Where-Object { $_.GetValue("DisplayName") -like "R for Windows*" } | ForEach-Object { $_.Name } }
  }
}
$before = @(Get-RRegistry)

"## B. 取得と展開（管理者権限なし）"
foreach ($v in $Versions) {
  $sw = [Diagnostics.Stopwatch]::StartNew()
  Invoke-WebRequest "$base/R-$v-windows.zip" -OutFile "$out\zip\R-$v.zip"
  $dl = $sw.Elapsed.TotalSeconds
  # zip の中は R-<版>/ から始まる
  [IO.Compression.ZipFile]::ExtractToDirectory("$out\zip\R-$v.zip", "$out\Programs\R")
  $size = (Get-ChildItem -Recurse -File "$out\Programs\R\R-$v" | Measure-Object -Sum Length).Sum
  "R $v：取得 $([math]::Round((Get-Item "$out\zip\R-$v.zip").Length / 1MB)) MB を $([math]::Round($dl, 1)) 秒、展開 $([math]::Round($sw.Elapsed.TotalSeconds - $dl, 1)) 秒、展開後 $([math]::Round($size / 1MB)) MB"
}

foreach ($v in $Versions) {
  $rs = "$out\Programs\R\R-$v\bin\Rscript.exe"
  "## C. R $v：起動、HTTPS、P3M のバイナリ、ソースからのビルド"
  & $rs "$PSScriptRoot\check.R" "$out\lib-$v" $Date
}

"## D. レジストリ（R-core と、アンインストールの登録）"
$after = @(Get-RRegistry)
$diff = Compare-Object $before $after
if ($diff) { "変化あり"; $diff } else { "変化なし（前後とも $($before.Count) 件）" }
