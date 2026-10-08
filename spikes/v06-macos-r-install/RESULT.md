# V6（と V1b の macOS 分）：macOS での R のインストール

実施：2026-10-08（GitHub Actions：macOS 26.6（arm64、`macos-latest`）と macOS 15.7（Intel、`macos-15-intel`）、Xcode のコマンドラインツールあり、P3M 2026-10-01）
再現：`.github/workflows/spikes.yml`（手動実行）、または Mac のこのフォルダで `./run.sh`。R は `out/R/R-<版>` に置き、`/Library/Frameworks` には触れない。

## 結論

**成功（IDE の認識を除く）。** Posit の portable なビルド（`https://cdn.posit.co/r/macos/R-<版>-macos-arm64.tar.gz`、Intel は `-macos.tar.gz`）を展開するだけで、管理者権限なし・画面なしで R を置ける。複数の版が共存し、P3M のバイナリパッケージもソースからのビルドも動く。rig の方式（CRAN の .pkg を入れて R.framework の版を切り替える）は管理者権限が要るので、portable なビルドの方が rok に合う。

## 確かめたこと

| 項目 | arm64 | Intel |
|---|---|---|
| 提供されている版 | 4.1.0〜4.6.1 の全版と next・devel（24） | 同じ |
| ない版 | 4.0.x、3.x | 同じ |
| 取得と展開（1つ） | 約 97 MB、2 秒＋展開 1 秒 | 約 97 MB、3 秒＋展開 3〜5 秒 |
| 展開後 | 162〜168 MB | 170〜175 MB |
| 隔離の属性（com.apple.quarantine） | 付かない（curl で取得） | 同じ |
| Gatekeeper（`spctl --assess`） | rejected（公証されていない）。ただし隔離の属性がないので、起動は妨げられない | 同じ |
| 起動・HTTPS（`url()`） | 動く | 動く |
| P3M のバイナリ（jsonlite・data.table） | 入って動く | 入って動く |
| ソースからのビルド（cli、C） | 成功（7〜10 秒） | 成功（27〜28 秒） |
| 共存（2つの版がそれぞれの R_HOME を使う） | はい | はい |

構成は `R-<版>/` が R_HOME（`bin/R`、`library/`、`etc/`、`lib/libR.dylib`）で、直下に `R`・`Rscript` もある。R.framework の形（`Versions/<版>/Resources/bin/R`）ではない。

## 設計への影響

1. **R 本体の入れ方（第6章）**：macOS でも portable なビルドを使い、ユーザー領域（rok の置き場所の `r/R-<版>`）に展開する（R 4.1.0 以降）。それより古い版は、Windows と同じく手動の導入を案内する → **要件の修正を提案する**
2. **隔離の属性**：rok が HTTP で取得する限り付かない。利用者がブラウザで取得したものを rok に渡す使い方は作らない
3. **IDE の認識**：macOS の Positron の標準の探索場所は `/Library/Frameworks/R.framework/Versions`（各版の `Resources/bin/R`）で、rok の置き場所は探さない。V7 の結果どおり、ワークスペースの設定（`interpreters.default` と `customBinaries` に `~` で始まるパス）で補う。実機の Positron では確かめていない（コードの読みによる）
4. **ビルドツール**：Xcode のコマンドラインツールが前提（`xcode-select -p` で判定できる）
