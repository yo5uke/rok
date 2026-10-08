# V7：Positron のワークスペース設定で、プロジェクトの R を指定できるか（手順書）

要件定義書 第6章「IDE の設定（ワークスペース単位）」と第13章 V7 の検証です。所要時間は30分ほどです。

## 確かめたいこと

rok は、プロジェクトの R を Positron に選ばせるために、`.vscode/settings.json`（ワークスペースの設定）に `.rok/bin/R`（プロジェクトの R へのリンク）を書く方式を考えています。そこで、次の点を確かめます。

1. R の探索の設定（`positron.r.customBinaries`・`positron.r.customRootFolders`）が、ワークスペースの設定として効くか
2. パスに相対パス（`.rok/bin/R`）や変数（`${workspaceFolder}`、`~`）を使えるか。公式の説明は「絶対パスを使う」としています
3. 既定で使う R を指定する設定があるか（要件の `interpreters.default` に当たるもの。公式の説明には見当たりません）
4. ワークスペースを信頼していない（制限モード）ときにどうなるか

結果に応じて、第6章の表の3つの分岐（自動で設定する／Git の管理下なら手動で案内する／何もしない）のどれにするかを決めます。

## 前提

- Positron から WSL の R を使える状態（Positron の WSL 接続、または Linux 上の Positron）
- この機械に、システムの R 4.6 があること（`R --version` で確認できます）

どちらかが難しい場合は、その旨を記録して V8 に進んでください。

## 準備（最初に1回）

WSL のターミナルで次を実行します。

```sh
cd ~/rok/spikes/v07-positron-settings
./prepare.sh
```

次のことを行います（数分かかります）。

- rok をビルドし、`~/.local/share/R/rok/` にバイナリと R パッケージを置く（`rok::setup()` と同じ場所）
- R 4.5 を `~/.local/share/R/rok/r/` に入れる（約 110 MB。管理者権限は使いません）
- 試験用のプロジェクト `~/rok-ide-check` を作る（R 4.5 のプロジェクト。`.rok/bin/R` は R 4.5 を指します）

試験用のプロジェクトの R をわざと 4.5 にしています。システムの R（4.6）が起動したら設定が効いていない、R 4.5 が起動・表示されたら効いている、と見分けられます。

## 手順

### 1. 設定名の確認

1. Positron で `~/rok-ide-check` を開きます（File > Open Folder）
2. コマンドパレットで「Preferences: Open Settings (UI)」を開き、検索欄に `positron.r` と入力します
3. 表示された設定の名前（ID）を**すべて**書き留めます。特に、既定の R・インタープリターを指定するものがあるかを見てください。`interpreters` でも検索してください
4. その設定がワークスペースのタブ（「Workspace」）でも設定できるかも書き留めます

### 2. 候補ごとの確認

ターミナルで候補を切り替え、Positron の表示を確かめます。候補 0 から 5 まで、それぞれ次を繰り返します。

```sh
cd ~/rok-ide-check
./v07-variant.sh 0      # 0 から 5 まで
```

1. Positron でコマンドパレットから「Developer: Reload Window」を実行します
2. 右上のインタープリターの選択（または「Interpreter: Select Interpreter Session」）を開き、R 4.5.x が一覧に出るか、出るならどのパスで表示されるかを見ます
3. R 4.5.x を選んで起動し、コンソールで `source("v07-probe.R")` を実行します（使っている R とパスが `out/v07-probe.txt` に残ります）

| 候補 | 中身 |
|---|---|
| 0 | 設定なし（比較の基準） |
| 1 | `positron.r.customBinaries`：`.rok/bin/R` の絶対パス |
| 2 | `positron.r.customBinaries`：相対パス `.rok/bin/R` |
| 3 | `positron.r.customBinaries`：`${workspaceFolder}/.rok/bin/R` |
| 4 | `positron.r.customRootFolders`：`~/.local/share/R/rok/r` の絶対パス |
| 5 | `positron.r.customRootFolders`：`~/.local/share/R/rok/r`（`~` のまま） |

手順 1 で既定の R を指定する設定が見つかった場合は、候補 1 の状態で、その設定に `.rok/bin/R` の絶対パスを `.vscode/settings.json` に手で書き足し、ウィンドウを開き直したときに R 4.5 が自動で起動するかも確かめてください。

### 3. ワークスペースの信頼

1. 候補 1 にした状態で、コマンドパレットの「Workspaces: Manage Workspace Trust」から、このフォルダを信頼しない（制限モード）にします
2. ウィンドウを開き直し、R 4.5.x が一覧に出るか、警告などが出るかを見ます
3. 最後に信頼する状態へ戻します

## 記録用紙

次の表を埋めて（分かる範囲で結構です）、チャットに貼ってください。`~/rok-ide-check/out/v07-probe.txt` は私が直接読みます。

```
Positron の版：
接続の形（WSL 接続 / Linux 上 / その他）：
手順1 の positron.r の設定の名前：
既定の R を指定する設定：あり（名前：　　　） / なし
ワークスペースのタブで設定できるか：

候補 | R 4.5 が一覧に出たか | 表示されたパス | 備考
0    |                      |                |
1    |                      |                |
2    |                      |                |
3    |                      |                |
4    |                      |                |
5    |                      |                |
既定の設定を書いた場合、R 4.5 が自動で起動したか：

制限モードで R 4.5 が一覧に出たか：　　　 警告など：
```

## 後片付け

確認が終わったら、次を消して構いません（V8 も終わってから）。

- `~/rok-ide-check`
- `~/.local/share/R/rok/r/4.5.*`（R 4.5。`~/rok/target/release/rok r uninstall 4.5.x` でも消せます）
- `~/.local/share/R/rok/bin`、`~/.local/share/R/rok/library`（rok のバイナリと R パッケージ）

## Windows の場合（Windows の Positron）

Windows の Positron と、Windows の R で同じことを確かめます。あわせて、V5 の「rok が入れた R（レジストリに登録しない）を Positron が認識するか」も見ます。手順の「確かめたいこと」と記録用紙は上と同じです。

### 前提

- Windows の Positron、システムの R 4.6（`C:\Program Files\R`）、PowerShell 7（`pwsh`）
- rok.exe（Windows 向けにビルドしたもの）。WSL から Windows 側にソースを写し、`cargo.exe build --release` で作れます

### 準備（最初に1回）

PowerShell で次を実行します（`<rok.exe>` はビルドした rok.exe のパス）。

```powershell
pwsh -File prepare-windows.ps1 -Rok <rok.exe>
```

次のことを行います。

- rok.exe と R パッケージを `R_user_dir("rok", "data")`（`%APPDATA%\R\data\R\rok`）に置く
- R 4.5 を rok で `%LOCALAPPDATA%\Programs\R\R-4.5.x` に入れる（Posit の portable なビルド、約 105 MB。レジストリには書かない）
- 試験用のプロジェクト `~\rok-ide-check` を作る（R 4.5 のプロジェクト。`.rok\R` は R 4.5 の R_HOME へのジャンクション）

Windows では、管理者権限なしにファイルへのシンボリックリンクを作れないため、Linux の `.rok/bin/R` の代わりに、R_HOME へのジャンクション `.rok\R` を置いています。

### 候補（`pwsh -File v07-variant.ps1 <番号>` で切り替える）

| 候補 | 中身 |
|---|---|
| 0 | 設定なし（比較の基準。**R 4.5.x が一覧に出れば、レジストリなしでも認識される**＝V5） |
| 1 | `positron.r.customBinaries`：`.rok\R\bin\R.exe` の絶対パス |
| 2 | `positron.r.customBinaries`：相対パス `.rok/R/bin/R.exe` |
| 3 | `positron.r.customBinaries`：`${workspaceFolder}/.rok/R/bin/R.exe` |
| 4 | `positron.r.customRootFolders`：`%LOCALAPPDATA%\Programs\R` の絶対パス |
| 5 | `positron.r.customRootFolders`：`~/AppData/Local/Programs/R`（`~` のまま） |

確かめ方（Reload Window、インタープリターの一覧、`source("v07-probe.R")`）とワークスペースの信頼の確認は、上の手順 1〜3 と同じです。

### 後片付け（Windows）

V8 も終わってから、次を消して構いません。

- `~\rok-ide-check`
- `%LOCALAPPDATA%\Programs\R\R-4.5.*`（`rok r uninstall 4.5.x` でも消せます。rok が入れた印のある R だけを消します）
- `%APPDATA%\R\data\R\rok`（rok.exe と R パッケージ）
