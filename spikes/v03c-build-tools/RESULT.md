# V3c：Linux のビルドツールの不足の判定

実施：2026-10-05（docker の `ubuntu:24.04`。ca-certificates だけを入れ、portable な R 4.4.2 を一般ユーザーで使う）
再現：`./run.sh`（判定の本体は `detect.sh`。v01b の `run.sh` を先に実行しておく）

## 結論

**成功。** ソースからのビルドに必要なものの不足を、root もネットワークも使わずに、ビルドの前に判定できた。判定したのは、言語ごとのコンパイラ、make、sysreqs の -dev パッケージである。提案したものだけを apt で入れると、5つのパッケージをすべてソースからビルドできた。ビルドの失敗のログから不足を読み取る方法は、最初に当たった不足しか分からないので、補助にとどめる。

## 方法（detect.sh。rok に組み込む判定の原型）

1. **必要な言語**：ソースの tar.gz の `src/` にあるファイルの拡張子（`.c`、`.cpp`・`.cc`、`.f`・`.f90`）と、DESCRIPTION の `LinkingTo`（Rcpp などは C++）から判定する。`src/` があれば、C と make は必ず要るとみなす。
2. **コンパイラ**：R が使うコマンドを `R_HOME/etc/Makeconf` の `CC`・`CXX`・`FC` から読み、`command -v` で探す。
   - `R CMD config CC` は、内部で make を使う。そのため make がない環境では `make: not found` を大量に出して使えなかった。
   - make のコマンドは Makeconf ではなく `etc/Renviron` で決まる（`MAKE=${MAKE-'make'}`）。最初はこれを見落とし、make を入れた後も「不足」と誤判定した。
3. **-dev パッケージ**：sysreqs（P3M の情報。ロックに記録する予定）の -dev パッケージについて、`dpkg-query -W -f='${db:Status-Abbrev}'` で入っているかを見る。dpkg の記録を読むだけなので、apt のリストも root も要らない。
4. **案内**：不足を apt のパッケージ名に直す（gcc → `gcc`、C++ → `g++`、Fortran → `gfortran`、make → `make`）。-dev パッケージと合わせて、1つの `sudo apt-get install` を示す。apt のリストがない環境では、`apt-get update` を添える（V3b）。

## 結果

対象は、cli・rlang（C）、Rcpp（C++）、quadprog（Fortran）、xml2（libxml2-dev が要る）。

| 段階 | 結果 |
|---|---|
| 0. 何も入れずにビルド | cli、xml2 とも失敗。ログから取り出せたのは `make: not found` だけ（最初に当たった不足） |
| 1. 判定 | 言語：C・C++・Fortran。不足：gcc、g++、gfortran、make、libxml2-dev |
| 2. 提案どおりに入れる | `apt-get install gcc g++ gfortran make libxml2-dev` |
| 3. もう一度判定 | 不足なし |
| 3. ソースからビルド | Rcpp 14 s、cli 9 s、quadprog 2 s、rlang 8 s、xml2 6 s。すべて成功し、読み込めた（quadprog の計算、xml2 の解析も動く） |

（xml2 は、最初は依存の rlang がなくて失敗した。これはテストの組み方の問題で、rlang を加えて解消した）

## 設計への影響

1. **重い同期の確認（第8章）**：ソースからのビルドが必要なパッケージがある場合は、確認を求める前にこの判定を行う。不足があれば、確認の画面に apt のコマンドを示す。
2. **Makeconf を直接読む**：rok は、ビルドツールの判定に `R CMD config` を使わない。
3. **失敗のログ**：判定が外れてビルドに失敗した場合に、ログから手がかり（`not found`、`.h: No such file`、pkg-config の失敗など）を取り出して示す。ただし、分かるのは最初の不足だけである。
4. **範囲**：V3b と同じく、apt（Debian・Ubuntu 系）での検証である。それ以外のディストリビューションは、第12章の未決事項のとおり。
5. **portable な R でのビルド（V1b）**：この検証の段階3で、portable な R でもソースからビルドできることを確かめた。
