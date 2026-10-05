# V9：renv との往復（Hash なしの renv.lock で restore）

実施：2026-10-05（**renv 1.3.0**、R 4.4.2（V1 の方法でユーザー領域に置いたもの）、Ubuntu 24.04）
再現：`./run.sh`（v01 の `run.sh` を先に実行しておく）。renv のキャッシュと設定は `out/` に置き、ユーザー全体の環境には触れない。

## 結論

**成功。** rok が出力する想定の renv.lock で、`renv::restore()` が動いた。この renv.lock は、Hash を含まず、P3M の日付つき URL を使い、日付が混在するパッケージは日付ごとのリポジトリを参照する形である。

- 6つのパッケージがすべて、ロックと同じ版で、バイナリとして入った
- パッケージごとのリポジトリの指定（`Repository`）と、日付つきの URL が守られた
- Hash がないことによる警告やエラーは出なかった

## 使った renv.lock

`out/with-repository.lock`（`run.sh` が作る）。要点は次のとおり。

- `R.Repositories` に、`P3M-2024-06-03`（`https://packagemanager.posit.co/cran/2024-06-03`）と `P3M-2023-06-01` の2つを並べた
- 各パッケージは `Source: Repository` と `Repository: <名前>` だけを持つ（Hash、Requirements はなし）
- data.table 1.14.8 と glue 1.6.2 は、2023-06-01 にしかない版である（2024-06-03 では、それぞれ 1.15.4 と 1.7.0）。これらには `P3M-2023-06-01` を指定した
- R6、cli、jsonlite、rlang は 2024-06-03 の版で、`P3M-2024-06-03` を指定した

## 結果

| | A：Repository を指定 | B：Repository 欄を消す（比較） |
|---|---|---|
| 版の一致 | 6/6 | 6/6 |
| バイナリ | 6/6（`[installed binary]`、手元でのビルドなし） | 6/6 |
| 取得元（DESCRIPTION の `RemoteRepos`） | data.table・glue は `__linux__/noble/2023-06-01`、他は `__linux__/noble/2024-06-03` | A と同じ |
| 警告 | 0 | 0 |

- **URL の変換**：renv は、ロックに書いたソースの URL（`/cran/<日付>`）を、自動で Linux バイナリの URL（`/cran/__linux__/noble/<日付>`）に変換した。そのため、ロックには OS 共通のソースの URL を書けばよい。
- **B の場合**：renv は、リポジトリを並べた順に探し、その版を持つリポジトリを見つけた。今回は `Repository` 欄がなくても結果は同じだった。ただし、同じ版が複数のリポジトリにある場合や、先頭のリポジトリのアーカイブにも古い版がある場合に、どれが選ばれるかは renv の探し方に依存する。明示する方が確実である。
- **注意（テストの組み方）**：プロジェクトを有効にしないまま `renv::restore()` を呼ぶと、`.libPaths()` の先頭（道具用のライブラリ）に入れてしまった。`library =` を明示する必要がある。

## 設計への影響

1. **export_renv の形式（第7章）**：要件どおり、Hash を含まない最小限の形式で足りる。リポジトリには P3M の日付つきの**ソースの URL** を書き、各パッケージに `Repository` を明示する。Linux のバイナリへの変換は renv が行う。
2. **CI の往復テスト（2-7）**：このスクリプトの形をもとにする。rok で作った環境を出力し、空のライブラリに `renv::restore(library = …)` して、版と取得元（`RemoteRepos`）を照合する。
3. **renv の版**：renv 1.3.0 で確かめた。renv の版が上がったときのために、往復テストで使う renv の版を記録して回す。
