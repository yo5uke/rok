# V2：バイナリパッケージを R を使わずに展開する（3 OS）

実施：2026-10-05（Ubuntu 24.04 / WSL2、V1 で置いた R 4.4.2、P3M 2026-10-01）
再現：`./run.sh`（v01 の `run.sh` を先に実行しておく）
Windows・macOS の分は、末尾の「Windows 分」「macOS 分」にある。

## 結論

**成功（Linux）。** P3M の Linux バイナリは、`tar -xzf` で展開するだけで使える。プロジェクトのライブラリにシンボリックリンクを置くだけで、`library()` が通る。`R CMD INSTALL` で入れた場合と、中身はバイト単位で同一だった。

## 確かめたこと

対象は、sf、data.table、R6、cli と、その依存（Depends・Imports）の12パッケージ。R に同梱の推奨パッケージは除く。

| 項目 | 結果 |
|---|---|
| 取得（User-Agent に R 4.4.2、8並列の curl） | 12件すべて `x-package-type: binary`。22 MB を 6.2 秒 |
| キャッシュへの展開（`<pkg>/<版>/4.4-noble/` に tar で展開） | 0.36 秒、展開後 44 MB |
| プロジェクトのライブラリ（`out/lib/<pkg>` → キャッシュへのシンボリックリンク） | 12/12 が `requireNamespace()` で読み込めた |
| `installed.packages()` | リンク経由の12件をすべて列挙する |
| 動作 | sf の `st_buffer()`・`st_area()`（GEOS）、data.table の集計が動く |
| `R CMD INSTALL` との比較（R6、data.table、sf） | `diff -r` で3件とも同一 |

## 注意点

- **パスの扱い**：読み込んだ名前空間のパス（`find.package()`）は、リンク先のキャッシュのパスになる。R がパスを正規化するためである。実行時に自分のインストール先へ書き込むパッケージは、共有のキャッシュに書き込むことになる（まれ）。
- **システムのライブラリの版**：このホストには、外部の apt リポジトリ由来の GEOS 3.14 が入っている。そのため sf が、「コンパイル時（3.12.1、noble の既定）と実行時で GEOS の版が違う」と警告した。P3M のバイナリは、ディストリビューションの既定のライブラリを前提にしている。
- **R の版の対応**：バイナリの `Built:` は `R 4.4.0`（パッチ版は 0）。R 4.4.2 で問題なく読み込めた。

## 設計への影響

1. **展開**：バイナリは R を起動せず、Rust で展開する（第9章の手段1が成立する）。`R CMD INSTALL` を経由する必要はない。
2. **キャッシュのキー**：応答ヘッダー `x-package-binary-tag`（`4.4-noble`、arm64 なら `4.4-noble-arm64`）を、キャッシュのキーの「R のマイナー版・OS」の部分に使える。
3. **リンク**：Linux では、パッケージのディレクトリへのシンボリックリンクで足りる。ライブラリから外すときは、リンクを消すだけでよい（キャッシュは残る）。
4. **速度**：取得の時間の大半は、リダイレクトと CDN の遅延だった。並列度の調整と評価は V13 で行う。

## Windows 分

実施：2026-10-08（Windows 11、R 4.6.1（`C:\Program Files\R`）、P3M 2026-10-01、Defender のリアルタイム保護は有効）
再現：`pwsh -File run-windows.ps1`（pwsh 7）。R は依存の一覧づくり（`windows-deps.R`）と読み込みの確認（`windows-check.R`）だけに使う。

### 結論

**成功（Windows）。** P3M の Windows のバイナリ（zip）は、展開するだけで使える。`R CMD INSTALL` で入れた場合と、中身は同一だった。

| 項目 | 結果 |
|---|---|
| 索引（`/cran/<日付>/bin/windows/contrib/4.6/PACKAGES`） | 全パッケージに `Hash`（zip の MD5）がある（25,140件） |
| 取得（sf、data.table、R6、cli と依存の12件、逐次） | 12件すべて `x-package-type: binary`、`x-package-binary-tag: 4.6-win`。66 MB を 20.6 秒 |
| MD5 の照合 | 12/12 が索引の `Hash` と一致 |
| 展開（`ZipFile.ExtractToDirectory`、zip の中は `<パッケージ>/` から始まる） | 1.68 秒、展開後 169 MB |
| 読み込み | 12/12 が `requireNamespace()` で読み込めた。`installed.packages()` も12件 |
| 動作 | sf の `st_buffer()`（GEOS 3.14.1）と GeoJSON の読み込み（GDAL 3.12.1）、data.table の4スレッドでの集計 |
| `R CMD INSTALL` との比較（R6、data.table、sf） | ファイルの一覧と SHA-256 が3件とも同一 |

### 注意点

- **転送先の名前**：P3M は `rspm-sync.rstudio.com/bin/4.6-win/<64桁>.zip` へ 307 で転送する。64桁は zip の SHA-256 ではない（照合には使えない）
- **大きさ**：sf などのバイナリは、GDAL などのライブラリを同梱するため大きい（sf だけで展開後 100 MB を超える）

### 設計への影響（Windows）

1. **展開**：zip を Rust で展開する（zip の crate が要る）。Linux と同じく、R を起動しない
2. **照合**：要件 第9章「完全性の検証」のとおり、索引の `Hash`（MD5）と照合できる（MD5 の crate が要る）
3. **キャッシュのキー**：`x-package-binary-tag`（`4.6-win`）が使える

## macOS 分

実施：2026-10-08（GitHub Actions の macOS、arm64 と Intel、R 4.6.1（portable なビルド、V6）、P3M 2026-10-01）
再現：`run-macos.sh`（`.github/workflows/spikes.yml`）

### 結論

**成功（macOS）。** P3M の macOS のバイナリ（tgz）は、展開するだけで使え、`R CMD INSTALL` で入れた場合と同一だった。

| 項目 | arm64（`sonoma-arm64`） | Intel（`big-sur-x86_64`） |
|---|---|---|
| 取得（12件、逐次） | 43 MB を 6.6 秒、すべて binary | 45 MB を 12.1 秒、すべて binary |
| MD5（索引の `Hash`） | 12/12 一致 | 10/12 一致（classInt・wk が不一致。後で取得し直すと一致した） |
| 展開 | 0.42 秒、103 MB | 2.5 秒、108 MB |
| 読み込み | 12/12 | 12/12 |
| `R CMD INSTALL` との比較（R6・data.table・sf） | 同一 | 同一 |

- バイナリの置き場所は、R のマイナー版と CPU ごとに P3M の `/__api__/status` の `macos_urls` が決める（R 4.6 の arm64 は `sonoma-arm64`、4.1〜4.5 は `big-sur-arm64`、Intel は `big-sur-x86_64`）
- キャッシュのキーには、応答の `x-package-binary-tag`（Intel は `4.6-macos`）が使える
- **MD5 の一時的な不一致**：P3M の索引とファイルが一時的に食い違うことがある。不一致のときは取得し直し、それでも合わなければ止める（照合を省かない）

