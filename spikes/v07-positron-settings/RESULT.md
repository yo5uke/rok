# V7（と V5 の Positron の認識）：Positron のワークスペース設定

実施：2026-10-08（Windows 11、Positron 2026.09.1、R 4.6.1（システム）と rok が入れた R 4.5.3（`%LOCALAPPDATA%\Programs\R\R-4.5.3`、レジストリなし））
再現：`pwsh -File prepare-windows.ps1 -Rok <rok.exe>` → `pwsh -File positron-log-check.ps1`。候補ごとに、別の設定フォルダ（`--user-data-dir`）で Positron を起動し、R 拡張のログ（`R Language Pack.log`）を読む。画面の操作はしていない。あわせて、R 拡張のコード（`extensions/positron-r/dist/extension.js`）を読んだ。
Linux（WSL）の Positron では確かめていない。設定の扱いは R 拡張の同じコードによるので、OS による違いは探索場所だけと考えられる。

## 結論

**ワークスペースの設定は効く。ただし、パスは絶対パスか `~` で始まるものだけで、相対パスと `${workspaceFolder}` は無視される。** 第6章の表では、2つめの分岐（効くが、絶対パスしか使えない）に当たる。ただし `~` が使えるため、rok が入れた R（利用者ごとに同じ相対位置）を指す設定は、利用者の名前を含まずに書ける。

## 確かめたこと

### 設定（R 拡張の package.json）

| 設定 | 適用範囲 | 意味 |
|---|---|---|
| `positron.r.customRootFolders` | resource（ワークスペースで可） | R を探す場所を足す |
| `positron.r.customBinaries` | resource | R の実行ファイルを足す |
| `positron.r.interpreters.default` | resource | 新しいワークスペースで既定にする R。ワークスペースで R を選んだ後は効かない |
| `positron.r.interpreters.exclude`・`override` | resource | 除く・限る |

どれも、`~`（`~/`・`~\`）をホームに展開した後、絶対パスでなければ「not absolute...ignoring」としてログに残して無視する（`${workspaceFolder}` は展開しない）。

### 探索場所（Windows）

既定で `C:\Program Files\R`、**`%LOCALAPPDATA%\Programs\R`**、レジストリ（`HKCU`・`HKLM` の `SOFTWARE\R-core\R64`）を見る。探索場所の各フォルダで `bin\x64\R.exe`、`bin\R.exe` を探す。PATH は Windows では既定で見ない。

### 候補ごとの結果

| 候補 | 設定 | 見つかった R | 設定の扱い | 開いた直後に起動した R |
|---|---|---|---|---|
| 0 | なし | 4.6.1、**4.5.3（rok が入れた R。理由：OS の標準の場所）** | — | 4.6.1 |
| 1 | `customBinaries`：`.rok\R\bin\R.exe` の絶対パス | 4.6.1、4.5.3（ジャンクションの先として） | 効く | 4.6.1 |
| 2 | `customBinaries`：`.rok/R/bin/R.exe` | 4.6.1、4.5.3 | 無視（not absolute） | 4.6.1 |
| 3 | `customBinaries`：`${workspaceFolder}/.rok/R/bin/R.exe` | 4.6.1、4.5.3 | 無視（not absolute） | 4.6.1 |
| 4 | `customRootFolders`：`%LOCALAPPDATA%\Programs\R` の絶対パス | 4.6.1、4.5.3 | 効く | 4.6.1 |
| 5 | `customRootFolders`：`~/AppData/Local/Programs/R` | 4.6.1、4.5.3 | 効く（`~` を展開） | 4.6.1 |
| 6 | `interpreters.default`：`.rok\R\bin\x64\R.exe` の絶対パス | 4.6.1、4.5.3 | 効く | なし（下記） |
| 7 | `interpreters.default`：R 4.5.3 の絶対パス | 4.6.1、4.5.3 | 効く（推奨の R になる） | なし（下記） |
| 8 | `interpreters.default`：`~/AppData/Local/Programs/R/R-4.5.3/bin/x64/R.exe` | 4.6.1、4.5.3 | 効く（`~` を展開） | なし（下記） |
| 9 | `interpreters.default`：`${workspaceFolder}/.rok/R/bin/x64/R.exe` | 4.6.1、4.5.3 | 無視（not absolute） | 4.6.1 |

- `interpreters.default` の R は、ワークスペースの「推奨の R」になる（ログ：`[recommendedWorkspaceRuntime] Recommending R runtime from 'positron.r.interpreters.default' setting`）。起動の仕方は `Implicit`（コードを実行するなど、R が要るときに起動する）で、開いた直後には起動しない（100 秒待っても起動しなかった）。設定がないときは、最新の R がすぐに起動した
- 「R が要るときに、推奨の R が起動する」ことは、画面の操作が要るため実地では確かめていない（コードの読みによる）

### ワークスペースの信頼

R 拡張は、信頼していないワークスペースを「limited」で扱い、その説明は「R cannot be started in untrusted folders」。制限モードでは R そのものが起動しないため、設定の扱い以前の問題になる。rok は信頼を回避しない（要件どおり）。

## 設計への影響

1. **第6章「IDE の設定」の分岐**：2つめ（効くが、絶対パスしか使えない）。ただし `~` が使えるので、次の方式なら設定ファイルに利用者固有のパスが入らず、Git で共有できる → **要件の修正を提案する**
   - Windows：rok が入れた R は標準の場所にあるので、探索の設定は要らない。既定の R を `interpreters.default` に `~/AppData/Local/Programs/R/R-<パッチ版>/bin/x64/R.exe` で書く
   - Linux：`customRootFolders` に `~/.local/share/R/rok/r`（rok の置き場所。各版のフォルダの `bin/R` を探す）、`interpreters.default` に `~/.local/share/R/rok/r/<パッチ版>/bin/R`
   - macOS：V6 の結果で決める
   - パッチ版はロックに記録されているので、共同研究者の間でも同じパスになる。その R がない人の環境では、Positron は「does not exist」としてログに残すだけで、害はない
2. **`.rok/bin/R`（プロジェクト内のリンク）**：設定から相対パスで指せないので、IDE の設定の目的では要らない。端末から使う入口として残すかは別に決める
3. **既定の R が開いた直後に起動しない**：`init()` の後は、IDE の再起動の指示（V8）か、R を選ぶ案内で、プロジェクトの R に切り替える（2-5）
