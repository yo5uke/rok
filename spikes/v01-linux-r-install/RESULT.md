# V1：Linux で権限なしに R を入れる

実施：2026-10-05（Ubuntu 24.04 / WSL2。まっさらな環境は docker の `ubuntu:24.04`）
再現：`./run.sh`（ホスト）→ `./container.sh`（まっさらな環境）。手順の本体は `relocate.sh`

## 結論

**成功。** Posit の R ビルドを、管理者権限なしでユーザー領域（`~/.local/share/R/rok/r/<版>` を含む任意の場所）に置ける。置いた R で、P3M のバイナリの導入と読み込み、ソースからのビルドができた。

ただし、R の実行に必要な**システムの共有ライブラリ**がなければ、R は起動しない。まっさらな Ubuntu 24.04 では、R 4.4.2 に19個の apt パッケージが必要だった。これらを入れるには、1回だけ sudo が必要になる。rok は、不足を判定してコマンドを示す（V3b と同じ方法）。

## 方法（relocate.sh）

1. `https://cdn.posit.co/r/<os>/R-<版>-<os>.tar.gz` を取得し、展開する。.deb ではなく tar.gz を使うので、dpkg も ar も要らない。
   - `<os>` は `ubuntu-2404` などで、P3M の `noble` とは名前の体系が違う。arm64 は末尾に `-arm64` が付く。
   - 最上位のディレクトリは `<版>/`（`bin/`、`lib/R/`、`share/`）。
2. 次のテキストのファイルに埋め込まれた `/opt/R/<版>` を、置いた場所に書き換える：`bin/R`、`lib/R/bin/R`、`lib/pkgconfig/libR.pc`、`lib/R/etc/Makeconf`（コメント行のみ）。
3. **Rscript はバイナリに R_HOME が埋め込まれていて、書き換えられない。** そこで、元のバイナリを `lib/R/bin/Rscript.orig` に移す。`bin/Rscript` と `lib/R/bin/Rscript` は、環境変数 `RHOME` を渡して元のバイナリを呼ぶシェルスクリプトに置き換える。
   - `lib/R/bin/Rscript` も置き換えが必要である。パッケージの configure は `${R_HOME}/bin/Rscript` を呼ぶことがある。
   - 元のバイナリを `lib/R/bin/exec/` に置いてはいけない。R CMD INSTALL がサブアーキテクチャとみなし、ビルドに失敗する（実際に失敗した）。
4. 書き換え漏れがないかを grep で確かめる。

展開と書き換えは 0.8 秒、tar.gz（63 MB）の取得は 2.6 秒だった。

## 確かめたこと

| 項目 | 結果 |
|---|---|
| `R --version`、`Rscript -e`、R の中から `R.home("bin")/Rscript` を呼ぶ | すべて動く |
| `capabilities()` | jpeg、png、tiff、tcltk、X11、cairo、ICU、libcurl がすべて TRUE（ホスト） |
| ソースからのビルド（cli、C のコードを含む） | 成功（8秒） |
| まっさらな環境・一般ユーザー・何も足さない | **起動しない**（`libblas.so.3` がない） |
| 同上・実行時ライブラリ19個だけを apt で入れる（コンパイラも -dev もなし） | P3M から R6 と data.table のバイナリを導入し、読み込めた |
| R 4.6.1 | 同じ手順で動く |
| R 3.6.3 | 書き換えは成功。`libpcre.so.3` がなく起動しない（`libpcre3` は noble で入手可能） |

まっさらな Ubuntu 24.04 で R 4.4.2 に必要な実行時パッケージ（`container.sh` の A で特定）：

```
libblas3 libcairo2 libcurl4t64 libdeflate0 libglib2.0-0t64 libgomp1 libicu74 libjpeg-turbo8
liblapack3 libpango-1.0-0 libpangocairo-1.0-0 libpng16-16t64 libreadline8t64 libtcl8.6
libtiff6 libtirpc3t64 libtk8.6 libx11-6 libxt6t64
```

このうち、起動に最低限必要なもの（`exec/R` と `libR.so` が参照するもの）は、`libblas3`、`libdeflate0`、`libgomp1`、`libicu74`、`libreadline8t64`、`libtirpc3t64` である。残りは、描画・tcltk・通信などのモジュールが使う。

参考：.deb の Depends には、ビルド用の g++、gcc、gfortran、make と -dev パッケージまで含まれる。R を使うだけなら過剰である。

## 設計への影響

1. **R 本体の取得**：tar.gz を Rust で展開し（flate2＋tar）、上の3か所を書き換える。.deb や rpm の扱いは要らない。
2. **Rscript**：ラッパーで `RHOME` を渡す。元のバイナリは `bin/exec/` の外に置く。
3. **システムライブラリ（第6章）**：R 本体の置き場所には権限が要らない。しかし、R の実行に必要な共有ライブラリが不足することがある。必要なものは R の版ごとに違う（3.6 系は libpcre3）。そのため、インストールの後に R 本体の共有ライブラリを V3b と同じ方法で調べ、不足があれば apt のコマンドを示す。sudo は実行しない → **要件の追記を提案する**。
4. **OS の名前の対応**：`/etc/os-release` から、Posit の R ビルドの名前（`ubuntu-2404`）と P3M の名前（`noble`）の両方を導く対応表が要る。
5. **入手可能な版**：`https://cdn.posit.co/r/versions.json` の `r_versions`（61件、`next`・`devel` を含む）。OS ごとの有無は載っていないので、HEAD で確かめる。

## 追記：portable な R ビルドの予備確認（2026-10-05、`./portable.sh`）

uvr の README から、Posit が **portable な R ビルド**（`manylinux_2_34`、試験的）を出していると分かった。https://github.com/rstudio/r-builds によると、次の性質を持つ。

- 主な共有ライブラリを同梱する
- 展開した場所を R 自身が実行時に検出する
- glibc 2.34 以上（Ubuntu 22.04 以降、RHEL 9 以降など）の Linux で動く
- 必要なのは ca-certificates と fontconfig だけとされている
- Windows（x86_64、R 3.6.3 以降）と macOS（R 4.1.0 以降）向けの portable なものもある

URL は `https://cdn.posit.co/r/manylinux_2_34/R-<版>-manylinux_2_34.tar.gz` で、arm64 は末尾に `-arm64` が付く。3.6.3、4.0.5、4.2.3、4.4.2、4.6.1 で取得できた（1つ約 100〜110 MB。4.4.2 では、通常のビルドの 63 MB に対して 108 MB で、約 7 割大きい）。

| 確認 | 結果 |
|---|---|
| まっさらな `ubuntu:24.04`、一般ユーザー、apt で何も入れない | 展開しただけで、`R --version` と `Rscript` が動いた。書き換えも Rscript のラッパーも不要。R 本体とモジュールが参照するライブラリに不足なし（0件）。`solve()` も動く。capabilities は cairo、tcltk、ICU、libcurl が TRUE |
| 同上で HTTPS（`url()`） | CA 証明書がないため失敗した。ca-certificates が要る（README の記載どおり） |
| ca-certificates・libcurl4t64・libgomp1 だけを入れ、P3M のバイナリを入れる | data.table、curl、jsonlite が入って動いた。curl パッケージが使うシステムの libcurl（8.5.0）と、R に同梱の libcurl（7.76.1）が共存し、HTTPS も通った |
| 同上で、libgomp1 を入れない場合 | data.table のバイナリが `libgomp.so.1` を見つけられず、読み込めない。portable な R が同梱するのは、自身が使うライブラリだけである。パッケージのバイナリが使うライブラリは、V3b の方法で判定する |

**予備確認の結論**：portable なビルドを使えば、R 本体の配置で「まっさらな環境では apt が1回必要」という制約（本書の上の結論）をほぼなくせる見込みがある。必要なのは ca-certificates だけになる。また、書き換えの手順（relocate.sh）も要らなくなる。

まだ確かめていないことは、次のとおりである。いずれも V1b として検証することを提案する。

- ソースからのビルド（`R CMD INSTALL` が、同梱のライブラリや Makeconf と噛み合うか）
- sf などのシステムのライブラリを使うパッケージとの組み合わせ
- Positron などの IDE が認識するか
- 「試験的」という位置づけ
- 同梱のライブラリの更新（OS の更新では直らず、R を入れ直す必要がある）
