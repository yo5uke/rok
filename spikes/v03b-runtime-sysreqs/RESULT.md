# V3b：Linux のシステム依存の実行時ライブラリの判定

実施：2026-10-05（docker の `ubuntu:24.04`。V1 で特定した R の実行時ライブラリだけを入れた状態から開始）
再現：`./run.sh`（v01 と v02 の `run.sh` を先に実行しておく）、`./apt-lists.sh`（apt のリストの有無による違い）

## 結論

**成功。** 展開済みのバイナリの共有ライブラリを ldd で調べると、実行時に不足するライブラリを正確に判定できた。判定には root もネットワークも要らない。

不足した soname は、実行時の apt パッケージに機械的に対応づけられた（apt のリストがある場合。ない場合は -dev パッケージの案内に戻す）。P3M の sysreqs（-dev パッケージ）に `apt-cache depends` をかけ、名前で照合する。提案したパッケージだけを入れると不足がなくなり、sf が動いた。

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

`apt-cache` は、手元のパッケージの一覧（`/var/lib/apt/lists`）を読む。引くだけなら root もネットワークも要らないが、一覧がない環境では働かない（次節）。

## apt のリストと最小イメージ（追記：2026-10-05、`./apt-lists.sh`）

`apt-cache depends` は、**apt のリスト（`/var/lib/apt/lists`）がないと、未インストールのパッケージを引けない。**

| 環境 | リスト | 結果 |
|---|---|---|
| `ubuntu:24.04`（そのまま、一般ユーザー） | なし（0件） | `apt-cache depends libgdal-dev` は `E: No packages found`、終了コード 100。`apt-cache policy` は何も出さず、終了コード 0 |
| `ubuntu:24.04`（root で `apt-get update` の後、一般ユーザー） | あり（52 MB） | 引ける。リストを作るのに root が要るが、引くだけなら一般ユーザーでよい |
| `rocker/geospatial:4.6.1`（イメージの作成時にリストを消している） | なし（0件） | インストール済みの libgdal-dev は引ける（dpkg の記録を読むため）。未インストールの libjags-dev は終了コード 100 |

- 判定（ldd）は、リストがなくても動く。最小イメージにも `ldd`（libc-bin）はある。
- 対応づけは、リストがないと働かない。不足を案内したい場面では、-dev パッケージも未インストールなのが普通なので、事実上引けない。
- docker のイメージでは、作成時にリストを消すのが一般的である。日常の WSL やデスクトップの Ubuntu では、リストがあるのが普通である。

**設計への影響（追加）**：`apt-cache depends` が終了コード 100 を返すか、候補が見つからない場合は、P3M の sysreqs の -dev パッケージをそのまま案内する。リストがない環境では、コマンドの前に `apt-get update` を付けて示す（`sudo apt-get update && sudo apt-get install -y …`）。

## 設計への影響

1. **status の判定（第5章 順位4）**：ELF と ld.so.cache を読むだけなので、ネットワークを使わず速い。Rust で実装すれば、ldd を起動するより速くできる。
2. **ロックに記録するもの（第7章）**：ディストリビューションごとの P3M の sysreqs（-dev パッケージ）を記録しておく。そうすれば、不足の案内をオフラインで組み立てられる。実行時のパッケージ名は、表示の時点で `apt-cache` から導く。
3. **R 本体（第6章）**：同じ方法で、R 本体（`exec/R`、`libR.so`、`modules/*.so`）の不足も判定できる（V1 を参照）。
4. **範囲**：この方法は apt（Debian・Ubuntu）が前提である。RHEL 系などでは、-dev パッケージを案内する方法にとどめる（要件は apt のみを対象としている）。
5. **検出の限界**：検出できるのは、共有ライブラリのリンク時の依存（ELF の NEEDED）だけである。実行時に `dlopen` で読み込むライブラリや、外部のコマンド（gdal-bin など）の不足は検出できない。
