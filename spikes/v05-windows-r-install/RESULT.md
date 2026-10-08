# V5（と V1b の Windows 分）：Windows での R のインストール

実施：2026-10-08（Windows 11、一般ユーザー、Rtools45（`C:\rtools45`）あり、P3M 2026-10-01）
再現：`pwsh -File run.ps1`（pwsh 7）。R は `out\Programs\R\R-<版>` に置き、実際の `%LOCALAPPDATA%\Programs\R` には触れない。

## 結論

**成功（Positron の認識を除く）。** Posit の portable なビルド（`https://cdn.posit.co/r/windows/R-<版>-windows.zip`）を展開するだけで、管理者権限なし・画面なし・レジストリへの書き込みなしで R を置ける。複数の版が共存し、P3M のバイナリパッケージもソースからのビルドも動く。Positron が認識するかは、2-1 の IDE での確認で見る。

## 確かめたこと

### A. 提供されている版

`versions.json` の61版に HEAD を送った。

| | 版 |
|---|---|
| あり（25） | next、devel、4.1.0〜4.6.1 の全版、3.6.3 |
| なし（36） | 4.0.0〜4.0.5、3.6.2 以前 |

arm64（`-windows-arm64.zip`）はない。大きさは1つ 96〜120 MB。

### B. 取得と展開

| R | 取得 | 展開 | 展開後 |
|---|---|---|---|
| 4.5.3 | 104 MB を 6.0 秒 | 4.1 秒 | 184 MB |
| 4.6.1 | 106 MB を 4.9 秒 | 4.0 秒 | 186 MB |

zip の中は `R-<版>/` から始まり、インストーラーで入れた R と同じ構成（`bin\R.exe`、`bin\x64\`、`etc\`、`library\` など）である。レジストリに登録する `bin\x64\RSetReg.exe` も入っているが、呼ばない。

### C. 動作（4.5.3 と 4.6.1 で同じ結果）

| 項目 | 結果 |
|---|---|
| 起動 | `Rscript.exe` が動き、`R.home()` は展開した場所になる |
| capabilities | jpeg・png・tiff・tcltk・cairo・ICU・libcurl など、すべて TRUE |
| HTTPS（`url()`） | 動く |
| P3M のバイナリ（jsonlite・data.table） | `install.packages(type = "binary")` で入り、動く |
| ソースからのビルド（cli、C） | Rtools45 で成功（30 秒台） |
| 共存 | 2つの版と、システムの R 4.6.1（`C:\Program Files\R`）が互いに干渉しない |

R 4.5 と 4.6 は、どちらも Rtools45 を使う。場所は `etc\Rcmd_environ` の `R_RTOOLS45_PATH="${RTOOLS45_HOME:-c:/rtools45}/..."` で決まる（`etc\x64\Makeconf` も同じ）。

### D. レジストリ

`HKCU`・`HKLM` の `Software\R-core` と、`HKCU` のアンインストールの登録（`R for Windows*`）を前後で比べ、変化はなかった（前後とも、システムの R による7件）。

## 設計への影響

1. **R 本体の入れ方（第6章）**：Windows では、portable なビルドを `%LOCALAPPDATA%\Programs\R\R-<版>` に展開する。インストーラーは使わず、レジストリにも登録しない（ユーザー全体の設定を書き換えない）→ **要件の修正を提案する**
2. **提供のない版（4.0.x、3.6.2 以前）**：portable なビルドがない。CRAN のインストーラーを `/CURRENTUSER` で使うと、レジストリ（アンインストールの登録など）に書き込む。扱いを決める必要がある → **判断を求める**
3. **ビルドツールの判定（Windows）**：`etc\Rcmd_environ` の `R_RTOOLS4x_PATH` から Rtools の場所（環境変数 `RTOOLS4x_HOME`、既定は `c:/rtools4x`）を読み、その有無で判定できる。Linux の Makeconf の判定と同じく、R を起動しない
4. **IDE の認識**：Positron と RStudio が `%LOCALAPPDATA%\Programs\R` の R を認識するか（レジストリなしで）は、2-1 で確かめる。認識しない場合は、`.vscode/settings.json`（第6章「IDE の設定」）で補う
