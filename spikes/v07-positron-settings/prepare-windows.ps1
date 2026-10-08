# V7・V8 の準備（Windows）：IDE で開く試験用のプロジェクトを作る（2-1）。prepare.sh の Windows 版。
#
# 1. rok.exe と R パッケージを、rok::setup() と同じ場所（R_user_dir("rok", "data")）に置く
# 2. R 4.5 を rok で入れる（Posit の portable なビルド、約 105 MB。%LOCALAPPDATA%\Programs\R\R-4.5.x。
#    レジストリには書かない。入っていれば何もしない）
# 3. R 4.5 のプロジェクトを作り、R6 を入れる。システムの R（4.6）と区別できるよう、プロジェクトの R は
#    わざと 4.5 にする
# 4. .rok\R（プロジェクトの R へのジャンクション）、Positron の設定の候補、確認用のスクリプトを置く
#
# 使い方（PowerShell 7）：pwsh -File prepare-windows.ps1 -Rok <rok.exe> [-Project <場所>]
#   -Rok を省くと、cargo でこのリポジトリの rok を release でビルドする
# 管理者権限は使わない。書き込む場所は、試験用のプロジェクト、R_user_dir("rok", ...)、
# %LOCALAPPDATA%\Programs\R\R-4.5.x だけ。
param(
  [string]$Rok,
  [string]$Project = (Join-Path $HOME "rok-ide-check"),
  [string]$RHome = (Get-ChildItem "$env:ProgramFiles\R" -Directory | Sort-Object Name | Select-Object -Last 1).FullName
)
$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$here = $PSScriptRoot
$repo = (Resolve-Path "$here\..\..").Path
$minor = "4.5"
$env:PATH = "$RHome\bin;$env:PATH"

"## 1. rok のバイナリと R パッケージ"
if (-not $Rok) {
  Push-Location $repo; cargo build --release --quiet; Pop-Location
  $Rok = "$repo\target\release\rok.exe"
}
$data = Rscript -e "cat(tools::R_user_dir('rok', 'data'))"
New-Item -ItemType Directory -Force "$data\bin", "$data\library\R-4.5", "$data\library\R-4.6" | Out-Null
Copy-Item $Rok "$data\bin\rok.exe" -Force
$rok = "$data\bin\rok.exe"
$build = New-Item -ItemType Directory -Force (Join-Path ([IO.Path]::GetTempPath()) "rok-rpkg-build")
Push-Location $build; R.exe CMD build --no-build-vignettes "$repo\rpkg" | Out-Null; Pop-Location
$tarball = (Get-ChildItem "$build\rok_*.tar.gz" | Select-Object -First 1).FullName
"   binary: $rok"

"## 2. R $minor"
& $rok r install $minor --yes 2>&1 | Select-Object -Last 2
$r45 = (Get-ChildItem "$env:LOCALAPPDATA\Programs\R" -Directory -Filter "R-$minor.*" | Sort-Object Name | Select-Object -Last 1).FullName
# The R package for each R that the checks use (setup() does this for the running R).
R.exe CMD INSTALL --no-test-load -l "$data\library\R-4.6" $tarball *> $null
& "$r45\bin\R.exe" CMD INSTALL --no-test-load -l "$data\library\R-4.5" $tarball *> $null
Remove-Item -Recurse -Force $build
"   R ${minor}: $r45"

"## 3. 試験用のプロジェクト：$Project"
if (Test-Path "$Project\rok.toml") {
  "   すでにあるので作り直さない（作り直すときは、そのフォルダを消してから実行する）"
} else {
  & $rok init $Project --r $minor --name rok-ide-check --yes 2>&1 | Select-Object -First 2
  & $rok --project $Project add R6 --yes 2>&1 | Select-Object -Last 1
}

"## 4. リンク・設定の候補・確認用スクリプト"
New-Item -ItemType Directory -Force "$Project\.rok", "$Project\.vscode\variants", "$Project\out" | Out-Null
# Windows では、管理者権限なしにファイルへのシンボリックリンクを作れないので、R_HOME へのジャンクションにする
if (Test-Path "$Project\.rok\R") { [IO.Directory]::Delete("$Project\.rok\R") }
New-Item -ItemType Junction -Path "$Project\.rok\R" -Target $r45 | Out-Null
Copy-Item "$here\probe.R" "$Project\v07-probe.R" -Force
Copy-Item "$here\variant-windows.ps1" "$Project\v07-variant.ps1" -Force
Copy-Item "$here\..\v08-ide-restart\restart.R" "$Project\v08-restart.R" -Force
Copy-Item "$here\..\v08-ide-restart\after.R" "$Project\v08-after.R" -Force
Copy-Item "$here\..\v08-ide-restart\scenario-windows.ps1" "$Project\v08-scenario.ps1" -Force
# Each start of R in the project is logged, to see whether a restart ran the startup hook.
$log = 'cat(format(Sys.time(), "%H:%M:%OS3"), Sys.getpid(), getwd(), sep = "\t", file = "out/startup.log", append = TRUE); cat("\n", file = "out/startup.log", append = TRUE)'
if (-not (Select-String -Path "$Project\.Rprofile" -SimpleMatch "out/startup.log" -Quiet)) {
  Add-Content "$Project\.Rprofile" $log
}

$abs = ("$Project\.rok\R\bin\R.exe" -replace '\\', '/')
$root = ("$env:LOCALAPPDATA\Programs\R" -replace '\\', '/')
function Write-Variant($n, $json) { Set-Content "$Project\.vscode\variants\$n.json" $json }
Write-Variant 1 "{ `"positron.r.customBinaries`": [`"$abs`"] }"
Write-Variant 2 '{ "positron.r.customBinaries": [".rok/R/bin/R.exe"] }'
Write-Variant 3 '{ "positron.r.customBinaries": ["${workspaceFolder}/.rok/R/bin/R.exe"] }'
Write-Variant 4 "{ `"positron.r.customRootFolders`": [`"$root`"] }"
Write-Variant 5 '{ "positron.r.customRootFolders": ["~/AppData/Local/Programs/R"] }'
& "$Project\v07-variant.ps1" 0 | Out-Null

""
"準備ができました。手順書（$here\README.md の「Windows の場合」）の続きに進んでください。"
"IDE で開くフォルダ：$Project"
