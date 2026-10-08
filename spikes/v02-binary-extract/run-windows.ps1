# V2（Windows 分）：P3M の Windows のバイナリ（zip）を、R を使わずに展開するだけで library() が通るかを確かめる。
# あわせて、索引の Hash（MD5）との照合と、R CMD INSTALL で入れた場合との比較を行う。
# 再現：pwsh -File run-windows.ps1 [-RHome <R のフォルダ>] [-Date <P3M の日付>]
# R は依存の一覧づくりと読み込みの確認だけに使う。ユーザーやサイトのライブラリは見ない。
param(
  [string]$RHome = (Get-ChildItem "$env:ProgramFiles\R" -Directory | Sort-Object Name | Select-Object -Last 1).FullName,
  [string]$Date = "2026-10-01"
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Add-Type -AssemblyName System.IO.Compression.FileSystem

$out = Join-Path $PSScriptRoot "out\windows"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$out\zip", "$out\lib", "$out\lib-r", "$out\empty" | Out-Null
$rscript = Join-Path $RHome "bin\Rscript.exe"
$env:R_LIBS_USER = "$out\empty"; $env:R_LIBS_SITE = "$out\empty"
$env:R_PROFILE_USER = "$out\empty\none"; $env:R_ENVIRON_USER = "$out\empty\none"

$minor = & $rscript "$PSScriptRoot\windows-minor.R"
$repo = "https://packagemanager.posit.co/cran/$Date/bin/windows/contrib/$minor"
"## R: $RHome（R $minor）、P3M: $repo"

# 対象：sf、data.table、R6、cli と、その依存（Depends・Imports）。R に同梱の推奨パッケージは除く
& $rscript "$PSScriptRoot\windows-deps.R" $repo "$out\packages.csv" sf data.table R6 cli
$pkgs = Import-Csv "$out\packages.csv"
"## 対象：$($pkgs.Count) パッケージ"

# 取得：P3M は 307 で実体へ転送する。種類（binary / source）は転送の応答のヘッダーで分かる
$sw = [Diagnostics.Stopwatch]::StartNew()
$bytes = 0; $md5ok = 0; $types = @{}
$handler = [Net.Http.HttpClientHandler]::new()
$handler.AllowAutoRedirect = $false
$client = [Net.Http.HttpClient]::new($handler)
foreach ($p in $pkgs) {
  $file = "$($p.package)_$($p.version).zip"
  $r = $client.GetAsync("$repo/$file").Result
  $type = $r.Headers.GetValues("x-package-type") | Select-Object -First 1
  $types[$type] = 1 + [int]$types[$type]
  Invoke-WebRequest $r.Headers.Location -OutFile "$out\zip\$file"
  $bytes += (Get-Item "$out\zip\$file").Length
  $md5 = (Get-FileHash -Algorithm MD5 "$out\zip\$file").Hash.ToLower()
  if ($md5 -eq $p.hash) { $md5ok++ } else { "MD5 mismatch: $file ($md5 / $($p.hash))" }
}
$sw.Stop()
"## 取得：$([math]::Round($bytes / 1MB, 1)) MB を $([math]::Round($sw.Elapsed.TotalSeconds, 1)) 秒（逐次）。種類：$(($types.GetEnumerator() | ForEach-Object { "$($_.Key) $($_.Value)" }) -join '、')"
"## MD5（索引の Hash）：$md5ok / $($pkgs.Count) が一致"

# 展開：zip の中は <パッケージ>/ から始まる。ライブラリにそのまま展開する
$sw = [Diagnostics.Stopwatch]::StartNew()
foreach ($z in Get-ChildItem "$out\zip\*.zip") {
  [IO.Compression.ZipFile]::ExtractToDirectory($z.FullName, "$out\lib")
}
$sw.Stop()
$size = (Get-ChildItem -Recurse -File "$out\lib" | Measure-Object -Sum Length).Sum
"## 展開：$([math]::Round($sw.Elapsed.TotalSeconds, 2)) 秒、展開後 $([math]::Round($size / 1MB, 1)) MB"

"## 読み込みと動作（R に展開先のライブラリだけを見せる）"
& $rscript "$PSScriptRoot\windows-check.R" "$out\lib" @($pkgs.package)

"## R CMD INSTALL との比較（R6、data.table、sf）"
foreach ($name in "R6", "data.table", "sf") {
  $p = $pkgs | Where-Object package -eq $name
  & (Join-Path $RHome "bin\R.exe") CMD INSTALL --library="$out\lib-r" "$out\zip\$($name)_$($p.version).zip" *> "$out\install-$name.log"
  $a = Get-ChildItem -Recurse -File "$out\lib\$name" | ForEach-Object {
    "$($_.FullName.Substring("$out\lib".Length)) $((Get-FileHash $_.FullName).Hash)" }
  $b = Get-ChildItem -Recurse -File "$out\lib-r\$name" | ForEach-Object {
    "$($_.FullName.Substring("$out\lib-r".Length)) $((Get-FileHash $_.FullName).Hash)" }
  $diff = Compare-Object $a $b
  if ($diff) { "${name}: 違いあり"; $diff | Format-Table -AutoSize | Out-String -Width 200 } else { "${name}: 同一（$($a.Count) ファイル）" }
}
