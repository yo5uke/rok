# V7・V5（Windows）：候補ごとに、別の設定フォルダで Positron を起動し、R 拡張のログから
# 見つかった R・設定の扱い・自動で起動した R を読む（画面の操作なしで確かめる）。
# 利用者の Positron とは設定フォルダ・拡張のフォルダ・プロセスが分かれる。ウィンドウは一時的に開いて閉じる。
# 使い方：pwsh -File positron-log-check.ps1 [-Variants 0,1,2] [-Project <試験用のプロジェクト>]
param(
  [int[]]$Variants = 0..9,
  [string]$Project = (Join-Path $HOME "rok-ide-check"),
  [int]$Wait = 45,
  [switch]$Keep
)
$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$work = Join-Path ([IO.Path]::GetTempPath()) "rok-v07-positron"
$out = Join-Path $Project "out"
New-Item -ItemType Directory -Force $out | Out-Null
$report = "$out\v07-positron-log.txt"
"Positron: $((Get-Item "$env:ProgramFiles\Positron\Positron.exe").VersionInfo.ProductVersion)  $(Get-Date)" | Add-Content $report

foreach ($v in $Variants) {
  $ud = "$work\ud-$v"
  Remove-Item -Recurse -Force $ud -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $ud, "$work\ext" | Out-Null
  & "$Project\v07-variant.ps1" $v | Out-Null
  $settings = if (Test-Path "$Project\.vscode\settings.json") { (Get-Content "$Project\.vscode\settings.json") -join " " } else { "(none)" }
  $p = Start-Process "$env:ProgramFiles\Positron\Positron.exe" -PassThru -ArgumentList @(
    "--user-data-dir", "`"$ud`"", "--logsPath", "`"$ud\logs`"", "--extensions-dir", "`"$work\ext`"",
    "--disable-workspace-trust", "--new-window", "`"$Project`"")
  Start-Sleep $Wait
  taskkill /T /F /PID $p.Id *> $null
  Start-Sleep 2
  # この試験の設定フォルダを使うプロセスだけを止める
  Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*$ud*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  $log = Get-ChildItem -Recurse "$ud\logs" -Filter "R Language Pack.log" | Select-Object -First 1
  $text = if ($log) { Get-Content $log.FullName } else { @() }
  $found = $text | Select-String -Pattern '"binpath": "(.*)"' | ForEach-Object { $_.Matches[0].Groups[1].Value -replace '\\\\', '\' } | Sort-Object -Unique
  $notes = $text | Select-String -Pattern "not absolute|customBinaries' to discover|customRootFolders' to scan|Default R interpreter path" |
    ForEach-Object { ($_.Line -replace '^\S+ \S+ \[\w+\] ', '').Trim() } | Sort-Object -Unique
  $session = $text | Select-String -Pattern 'Creating language client (R [\d.]+)' | Select-Object -First 1 | ForEach-Object { $_.Matches[0].Groups[1].Value }
  $block = @("## 候補 $v：$settings", "  見つかった R：", ($found | ForEach-Object { "    $_" }), "  設定の扱い：", ($notes | ForEach-Object { "    $_" }), "  自動で起動した R：$session", "")
  $block | Add-Content $report
  $block
}
& "$Project\v07-variant.ps1" 0 | Out-Null
if (-not $Keep) { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
"結果：$report"
