# V11：Windows で、プロジェクトのライブラリをキャッシュへのジャンクションで作れるか。
# R の読み込み、Defender が有効な状態での速度、読み込み中の削除、壊れたジャンクション、日本語を含むパスを確かめる。
# OneDrive の同期フォルダでの確認は、ファイルがクラウドに上がるため、ここでは行わない。
# 再現：pwsh -File run.ps1 [-RHome <R のフォルダ>] [-Date <P3M の日付>]（pwsh 7 が必要）
param(
  [string]$RHome = (Get-ChildItem "$env:ProgramFiles\R" -Directory | Sort-Object Name | Select-Object -Last 1).FullName,
  [string]$Date = "2026-10-01"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -AssemblyName System.IO.Compression.FileSystem

$out = Join-Path $PSScriptRoot "out"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$out\zip", "$out\cache", "$out\lib", "$out\direct", "$out\empty" | Out-Null
$rscript = Join-Path $RHome "bin\Rscript.exe"
$env:R_LIBS_USER = "$out\empty"; $env:R_LIBS_SITE = "$out\empty"
$env:R_PROFILE_USER = "$out\empty\none"; $env:R_ENVIRON_USER = "$out\empty\none"
$minor = & $rscript "$PSScriptRoot\..\v02-binary-extract\windows-minor.R"
$repo = "https://packagemanager.posit.co/cran/$Date/bin/windows/contrib/$minor"
"## R $minor、Defender のリアルタイム保護：$((Get-MpComputerStatus).RealTimeProtectionEnabled)"

& $rscript "$PSScriptRoot\..\v02-binary-extract\windows-deps.R" $repo "$out\packages.csv" tidyverse
$pkgs = Import-Csv "$out\packages.csv"
$sw = [Diagnostics.Stopwatch]::StartNew()
$pkgs | ForEach-Object -ThrottleLimit 16 -Parallel {
  $ProgressPreference = "SilentlyContinue"
  Invoke-WebRequest "$using:repo/$($_.package)_$($_.version).zip" -OutFile "$using:out\zip\$($_.package)_$($_.version).zip"
}
"## 取得：$($pkgs.Count) パッケージ、$([math]::Round((Get-ChildItem "$out\zip" | Measure-Object -Sum Length).Sum / 1MB)) MB を $([math]::Round($sw.Elapsed.TotalSeconds, 1)) 秒（16並列）"

"## A. キャッシュへの展開と、ジャンクションでのライブラリ"
$sw = [Diagnostics.Stopwatch]::StartNew()
foreach ($p in $pkgs) {
  [IO.Compression.ZipFile]::ExtractToDirectory("$out\zip\$($p.package)_$($p.version).zip", "$out\cache\$($p.package)\$($p.version)")
}
$extract = $sw.Elapsed.TotalSeconds
$sw = [Diagnostics.Stopwatch]::StartNew()
foreach ($p in $pkgs) {
  New-Item -ItemType Junction -Path "$out\lib\$($p.package)" -Target "$out\cache\$($p.package)\$($p.version)\$($p.package)" | Out-Null
}
$link = $sw.Elapsed.TotalSeconds
$files = Get-ChildItem -Recurse -File "$out\cache"
"展開：$([math]::Round($extract, 1)) 秒（$($files.Count) ファイル、$([math]::Round(($files | Measure-Object -Sum Length).Sum / 1MB)) MB）。ジャンクション $($pkgs.Count) 個：$([math]::Round($link * 1000)) ミリ秒"
$longest = ($files | ForEach-Object { $_.FullName.Length - "$out\cache\".Length } | Measure-Object -Maximum).Maximum
"キャッシュ内の最長の相対パス：$longest 文字（キャッシュの置き場所の長さに足される。MAX_PATH は 260）"
foreach ($p in $pkgs) {
  [IO.Compression.ZipFile]::ExtractToDirectory("$out\zip\$($p.package)_$($p.version).zip", "$out\direct")
}

"## B. R での読み込み（ジャンクションのライブラリと、実体のライブラリ）"
& $rscript "$PSScriptRoot\load.R" "$out\lib" "$out\direct"

"## C. 読み込み中の削除（R が dplyr の DLL を読み込んだまま）"
$r = Start-Process $rscript -ArgumentList "-e", "`".libPaths(c('$($out -replace '\\','/')/lib', .Library)); library(dplyr); Sys.sleep(20)`"" -PassThru -WindowStyle Hidden
Start-Sleep 8
$dll = Get-ChildItem "$out\cache\dplyr" -Recurse -Filter dplyr.dll | Select-Object -First 1
try { Remove-Item $dll.FullName; "キャッシュの DLL の削除：できた" } catch { "キャッシュの DLL の削除：できない（$($_.Exception.Message.Trim())）" }
try { [IO.Directory]::Delete("$out\lib\dplyr"); "ジャンクションの削除：できた（キャッシュの実体は残る：$(Test-Path "$out\cache\dplyr\*\dplyr\DESCRIPTION")）" } catch { "ジャンクションの削除：できない（$($_.Exception.Message.Trim())）" }
$ver = ($pkgs | Where-Object package -eq dplyr).version
New-Item -ItemType Junction -Path "$out\lib\dplyr" -Target "$out\cache\dplyr\$ver\dplyr" | Out-Null
"ジャンクションの張り直し：できた"
$r.WaitForExit()

"## D. 壊れたジャンクション（リンク先のキャッシュを消す）"
[IO.Directory]::Delete("$out\cache\R6", $true)
$j = Get-Item "$out\lib\R6" -Force
"Get-Item：LinkType=$($j.LinkType)、Target=$($j.Target)、Test-Path=$(Test-Path "$out\lib\R6")、Exists=$($j.Exists)"
& $rscript "$PSScriptRoot\broken.R" "$out\lib"
[IO.Directory]::Delete("$out\lib\R6")
"壊れたジャンクションの削除（Directory.Delete、再帰なし）：できた"

"## E. 日本語と空白を含むパス"
$jp = "$out\キャッシュ 日本語"
[IO.Compression.ZipFile]::ExtractToDirectory("$out\zip\cli_$(($pkgs | Where-Object package -eq cli).version).zip", "$jp")
New-Item -ItemType Directory -Force "$out\ライブラリ" | Out-Null
New-Item -ItemType Junction -Path "$out\ライブラリ\cli" -Target "$jp\cli" | Out-Null
& $rscript "$PSScriptRoot\unicode.R" "$out\ライブラリ"
