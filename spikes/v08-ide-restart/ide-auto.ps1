# V8（自動、Windows）：Positron と RStudio を、利用者の設定とは別の場所を使って起動し、auto.R で
# 再起動の指示とその結果を記録する。画面の操作は要らない（ウィンドウは一時的に開いて閉じる）。
#   Positron：--user-data-dir・--extensions-dir・--logsPath を作業用の場所に向ける
#   RStudio：RSTUDIO_CONFIG_HOME・RSTUDIO_DATA_HOME を作業用の場所に向け、RSTUDIO_WHICH_R でプロジェクトの R を選ぶ
# 使い方：pwsh -File ide-auto.ps1 [-Project <試験用のプロジェクト>] [-Ide Positron,RStudio]
param(
  [string]$Project = (Join-Path $HOME "rok-ide-check"),
  [string[]]$Ide = @("Positron", "RStudio"),
  [int]$Wait = 60
)
$ErrorActionPreference = "Continue"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$work = Join-Path ([IO.Path]::GetTempPath()) "rok-v08-auto"
Copy-Item "$PSScriptRoot\auto.R" "$Project\v08-auto.R" -Force
$line = 'if (nzchar(Sys.getenv("ROK_V08_AUTO"))) source("v08-auto.R")'
if (-not (Select-String -Path "$Project\.Rprofile" -SimpleMatch $line -Quiet)) { Add-Content "$Project\.Rprofile" $line }
$r45 = (Get-Item "$Project\.rok\R").Target
$env:ROK_V08_AUTO = "1"

function Stop-Tree($p, $pattern) {
  taskkill /T /F /PID $p.Id *> $null
  Start-Sleep 2
  Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*$pattern*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

foreach ($name in $Ide) {
  Remove-Item -Force "$Project\out\v08-auto.restarted", "$Project\out\startup.log" -ErrorAction SilentlyContinue
  "## $name" | Add-Content "$Project\out\v08-auto.txt"
  $dir = "$work\$name"
  Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $dir | Out-Null
  if ($name -eq "Positron") {
    # interpreters.default で選んだ R は、使うときまで起動しない（Implicit）。セッションをすぐに
    # 始めるため、設定なし（V7 の候補 0：最新の R が起動する）で開く
    & "$Project\v07-variant.ps1" 0 | Out-Null
    $p = Start-Process "$env:ProgramFiles\Positron\Positron.exe" -PassThru -ArgumentList @(
      "--user-data-dir", "`"$dir\ud`"", "--logsPath", "`"$dir\logs`"", "--extensions-dir", "`"$dir\ext`"",
      "--disable-workspace-trust", "--new-window", "`"$Project`"")
  } else {
    $rproj = "$Project\rok-ide-check.Rproj"
    if (-not (Test-Path $rproj)) { Set-Content $rproj "Version: 1.0`n" }
    $env:RSTUDIO_CONFIG_HOME = "$dir\config"; $env:RSTUDIO_DATA_HOME = "$dir\data"
    $env:RSTUDIO_WHICH_R = "$r45\bin\x64\R.exe"
    $p = Start-Process "$env:ProgramFiles\RStudio\rstudio.exe" -PassThru -ArgumentList @("`"$rproj`"")
  }
  Start-Sleep $Wait
  Stop-Tree $p $dir
  if ($name -eq "RStudio") { Stop-Tree $p "rok-ide-check.Rproj"; Remove-Item Env:\RSTUDIO_CONFIG_HOME, Env:\RSTUDIO_DATA_HOME, Env:\RSTUDIO_WHICH_R }
  if ($name -eq "Positron") { & "$Project\v07-variant.ps1" 0 | Out-Null }
  "startup.log（.Rprofile が走った回）：" | Add-Content "$Project\out\v08-auto.txt"
  if (Test-Path "$Project\out\startup.log") { Get-Content "$Project\out\startup.log" | Add-Content "$Project\out\v08-auto.txt" }
}
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
Get-Content "$Project\out\v08-auto.txt"
