# V1b：portable な R ビルド（Linux 分）

実施：2026-10-05（docker の `ubuntu:24.04`、一般ユーザー（ホストと同じ uid））
再現：`./run.sh`（A と C）。ソースからのビルド（B）は `../v03c-build-tools/run.sh` で確かめた。
予備確認は `../v01-linux-r-install/portable.sh` と、その RESULT.md の追記にある。
Windows・macOS 向けの portable なビルドと、IDE（Positron）の認識は、まだ確かめていない（2-1、2-2 で行う）。

## 結論

**成功（Linux）。** Posit の portable なビルド（`https://cdn.posit.co/r/manylinux_2_34/R-<版>-manylinux_2_34.tar.gz`、試験的）について、次のことを確かめた。

- 展開するだけで、まっさらな Ubuntu 24.04 で一般ユーザーが使える。apt で入れるものはなく、書き換えも要らない
- ソースからのビルドができる
- システムのライブラリを使うパッケージ（P3M のバイナリ）と組み合わせても動く

V1 の方法（ディストリビューション向けのビルドを展開して書き換える）より手間が少なく、必要なシステムのパッケージもほぼない。

## 確かめたこと

### A. apt で何も入れない状態

| R | 不足する共有ライブラリ | solve() | R の中から Rscript | tcltk | cairo |
|---|---|---|---|---|---|
| 3.6.3 | 0 | 動く | 動く | TRUE | TRUE |
| 4.4.2 | 0 | 動く | 動く | TRUE | TRUE |
| 4.6.1 | 0 | 動く | 動く | TRUE | TRUE |

- 同梱のライブラリ（OpenSSL、libcurl、libxml2、libgomp、libgfortran、ICU、cairo、pango、tcl/tk など66件）は、`lib/R/lib/.libs` に、ハッシュつきの別名（例：`libssl-78885cee.so.3`）で置かれている。RUNPATH（`$ORIGIN/.libs`）で参照される。Python の manylinux の wheel と同じ方式である。
- SBOM（`lib/R/sbom.cdx.json`）に、同梱のライブラリの一覧がある。
- Makeconf の BLAS などのパスは、展開した場所に合わせて解決される。`/opt/R` が残るのは、configure の記録のコメント行だけである。
- HTTPS には ca-certificates が要る（R の `url()` は、CA 証明書がないと失敗する）。

### B. ソースからのビルド（V3c の run.sh）

ビルドツール（gcc、g++、gfortran、make）と libxml2-dev を入れたうえで、次の5つを portable な R 4.4.2 でソースからビルドし、読み込めた。

- cli、rlang（C）
- Rcpp（C++）
- quadprog（Fortran）
- xml2（システムの libxml2 にリンクする）

### C. システムのライブラリを使うパッケージ（P3M のバイナリ、R 4.4.2）

実行時のライブラリ（libgomp1、libcurl4t64、libxml2、GDAL・GEOS・PROJ、freetype、harfbuzz など）だけを入れた状態で確かめた。

| 処理 | 結果 |
|---|---|
| R の `url()` で HTTPS（同梱の libcurl 7.76.1） | 動く |
| data.table を OpenMP の4スレッドで（同梱とシステムの libgomp が共存） | 動く |
| curl パッケージで HTTPS（システムの libcurl 8.5.0） | 動く |
| openssl の sha256 と RSA の鍵の生成（システムの OpenSSL 3.0.13） | 動く |
| xml2 の解析と XPath（システムの libxml2） | 動く |
| sf の GeoJSON の読み込み・座標変換・バッファ（GDAL・PROJ・GEOS） | 動く |
| ragg の PNG の描画と文字（システムの freetype・harfbuzz） | 動く |

途中の失敗は3回あった。いずれも、用意した実行時ライブラリの一覧の漏れ（libharfbuzz0b、libfribidi0、libwebpmux3）によるもので、portable な R とは関係しない。パッケージのバイナリが使うライブラリは、V3b の方法で判定する。

## 注意点

- **試験的な提供**：Posit は「experimental」としている。提供が変わったり止まったりした場合に備え、V1 の方法（ディストリビューション向けのビルドを書き換える）を残しておく。
- **glibc 2.34 以上**：Ubuntu 22.04 以降、Debian 12 以降、RHEL 9 以降。Ubuntu 20.04 と RHEL 8 では使えないので、V1 の方法になる。
- **同梱のライブラリの更新**：OS の更新では直らない。OpenSSL などの脆弱性の修正を受けるには、R を入れ直す必要がある（SBOM で確認できる）。
- **大きさ**：4.4.2 で 108 MB（ディストリビューション向けは 63 MB）。

## 設計への影響

1. **R 本体の入れ方（第6章）**：Linux では、portable なビルドを第一の選択肢にする。V1 の書き換えの手順と Rscript のラッパーは、glibc の古い環境などに備えた代替手段とする → **要件の修正を提案する**。
2. **R 本体の不足の判定（第6章）**：portable なビルドでも、配置の後に V3b の方法で判定する仕組みは残す。必要になるのは ca-certificates くらいで、判定はほぼ常に「不足なし」になる。
3. **Windows・macOS（2-2）**：portable なビルド（Windows x86_64 は R 3.6.3 以降、macOS は R 4.1.0 以降）を、V5・V6 の比較の対象に加える。
