# V8：IDE への R の再起動の指示

実施：2026-10-08（Windows 11、Positron 2026.09.1（ark 0.1.252）、RStudio for Windows、rok の試験用のプロジェクト）
再現：V7 の準備の後、`pwsh -File ide-auto.ps1`。IDE ごとに、利用者の設定とは別の場所（Positron は `--user-data-dir` など、RStudio は `RSTUDIO_CONFIG_HOME`・`RSTUDIO_DATA_HOME`）で起動し、`auto.R` がセッション開始のフック（`positron.session_init`、`rstudio.sessionInit`）から base R で再起動を指示して、前後を `out/v08-auto.txt` に記録する。画面の操作はしていない。
RStudio の起動で、`%APPDATA%\RStudio\config.json`（ウィンドウの位置など）は通常の起動と同じく更新されたが、R の選択（`rExecutablePath`）は変わらなかった。

## 結論

**成功。** Positron と RStudio のどちらでも、base R から `.rs.api.restartSession()` で R の再起動を指示できる（rstudioapi は要らない）。再起動の後は新しいプロセスになり、`.Rprofile`（rok の起動フック）が走り、作業ディレクトリは保たれる。**グローバル環境のオブジェクトは残らない。**

## 確かめたこと

| | Positron | RStudio |
|---|---|---|
| `.rs.api.restartSession` があるか | ある（ark が定義。中身は `.ps.ui.executeCommand("workbench.action.language.runtime.restartActiveSession")`） | ある |
| 再起動にかかった時間 | 約 1.1 秒 | 約 2.6 秒 |
| 新しいプロセスか | はい（pid 34720 → 8868） | はい（pid 48656 → 8732） |
| `.Rprofile` が走ったか | はい | はい |
| 作業ディレクトリ | 保たれた（ドライブ文字の大小だけ変わった） | 保たれた |
| グローバル変数 | 残らなかった | 残らなかった |
| 確認のダイアログ | 出なかった | 出なかった |
| 起動した R | 4.6.1（設定なしで開いたため。プロジェクトは 4.5 なので、rok は空の隔離ライブラリにした） | 4.5.3（`RSTUDIO_WHICH_R`）。プロジェクトのライブラリが有効 |

ほかに、ark には `.ps.ui.openWorkspace(path, newSession)`（フォルダを開く）、`.ps.ui.executeCommand(command)`（任意のコマンド）がある。RStudio の `.rs.api.openProject()` に当たる。

2-4 の起動時の表示（軽い同期・重い同期の3択・R の版の違いの案内など）が IDE のコンソールでどう見えるかは、画面を見る必要があり、まだ確かめていない（手順書の B）。

## 設計への影響

1. **init 後の再起動（第5章）**：`exists(".rs.api.restartSession")` なら呼ぶ、という1つの書き方で、Positron と RStudio の両方に再起動を指示できる。どちらでもない（端末の R など）ときは、要件どおり、その場で切り替えるか再起動を案内する
2. **事前の確認（第5章）**：再起動でグローバル環境のオブジェクトは失われるので、要件どおり、オブジェクトがある場合だけ確認する
3. **R の版を変えるとき**：再起動は「今の R」で起き直すだけで、別の版には切り替わらない（Positron では、既定の R の設定も開いた直後には効かない。V7）。版が違うときは、IDE で R を選ぶ案内が要る（2-5）
