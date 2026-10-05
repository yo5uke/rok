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
