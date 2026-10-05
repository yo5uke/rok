# V3b：Linux のシステム依存の実行時ライブラリの判定

実施：2026-10-05（docker の `ubuntu:24.04`。V1 で特定した R の実行時ライブラリだけを入れた状態から開始）
再現：`./run.sh`（v01 と v02 の `run.sh` を先に実行しておく）

## 結論

**成功。** 展開済みのバイナリの共有ライブラリを ldd で調べると、実行時に不足するライブラリを正確に判定できた。判定には root もネットワークも要らない。

不足した soname は、実行時の apt パッケージに機械的に対応づけられた。P3M の sysreqs（-dev パッケージ）に `apt-cache depends` をかけ、名前で照合する。提案したパッケージだけを入れると不足がなくなり、sf が動いた。

## 確かめたこと

| 手順 | 結果 |
|---|---|
| P3M の sysreqs（12パッケージ分） | s2：libabsl-dev、cmake、libssl-dev／sf：libgdal-dev、gdal-bin、libgeos-dev、libproj-dev、libsqlite3-dev／units：libudunits2-dev（9個） |
| 判定（10個の `.so` を ldd、74 ms） | 不足は sf の `libgdal.so.34`、`libgeos_c.so.1`、`libproj.so.25` と、units の `libudunits2.so.0` だけ。s2 は不足なし |
| 対応づけ | `libgdal34t64`、`libgeos-c1t64`、`libproj25`、`libudunits2-0`（4個） |
| 4個だけを入れた後 | 不足は 0。`library(sf)` と `st_buffer()` が動く |

sysreqs をそのまま使うと、9個を案内することになる。その中には、次のものが含まれる。
- 実行には要らないビルド用のツール（cmake、gdal-bin）
- 不足が出なかったパッケージ用のもの（s2 用の libabsl-dev と libssl-dev）
- 直接の不足として現れなかったもの（libsqlite3-dev）

判定と対応づけを使うと、本当に不足する4個だけを案内できた。

## 方法

1. **判定**：プロジェクトのライブラリにある各パッケージの `libs/*.so` について、参照するライブラリ（ELF の NEEDED）が、システムのライブラリの探索先（ld.so.cache）にあるかを調べる。`libR.so` は R の外からは見つからないので、R_HOME/lib を探索先に加えるか、除外する。
2. **対応づけ**：不足があった R パッケージについて、sysreqs の -dev パッケージごとに `apt-cache depends <dev>` を実行する。その Depends のうち、`lib` で始まり `-dev` で終わらないものを候補にする。
3. **照合**：soname と候補の名前を英数字だけに正規化し、前者が後者に含まれれば一致とする。
   - `libgdal.so.34` → `gdal34`、`libgdal34t64` → `gdal34t64`
   - `libgeos_c.so.1` → `geosc1`、`libgeos-c1t64` → `geosc1t64`
   - `libudunits2.so.0` → `udunits20`、`libudunits2-0` → `udunits20`
   - 一致しない場合は、-dev パッケージを案内する。過剰にはなるが、確実に入る。

`apt-cache` は、手元のパッケージの一覧（`/var/lib/apt/lists`）を読むので、root もネットワークも要らない。

## 設計への影響

1. **status の判定（第5章 順位4）**：ELF と ld.so.cache を読むだけなので、ネットワークを使わず速い。Rust で実装すれば、ldd を起動するより速くできる。
2. **ロックに記録するもの（第7章）**：ディストリビューションごとの P3M の sysreqs（-dev パッケージ）を記録しておく。そうすれば、不足の案内をオフラインで組み立てられる。実行時のパッケージ名は、表示の時点で `apt-cache` から導く。
3. **R 本体（第6章）**：同じ方法で、R 本体（`exec/R`、`libR.so`、`modules/*.so`）の不足も判定できる（V1 を参照）。
4. **範囲**：この方法は apt（Debian・Ubuntu）が前提である。RHEL 系などでは、-dev パッケージを案内する方法にとどめる（要件は apt のみを対象としている）。
