# V4：P3M が古い R の版向けにバイナリを提供する範囲

実施：2026-10-05（P3M 2026.09.0。今のディストリビューション＝Ubuntu 24.04 noble）
再現：`python3 availability.py`（提供範囲の地図）、`python3 compare_probes.py`（調べるパッケージを変えた比較）、`python3 per_version.py`（同じ版で日付を変えた比較）、`./runtime.sh`（noble での動作。v01 を先に実行しておく）

## 結論

**提供範囲は「R のマイナー版ごとの日付の範囲」としては決まらない。** バイナリがあるかどうかは、（パッケージの版、R のマイナー版、ディストリビューション）の組ごとに異なる。P3M は、過去の日付のパッケージにも、新しい R の版やディストリビューション向けのバイナリをさかのぼってビルドしている。ただし、その範囲はパッケージによってまちまちである。

**届いたバイナリは、今のディストリビューションで動く。** 古い R（3.6.3）、古い日付（2020年）、R の公開前にさかのぼってビルドされたものも含めて、noble で読み込めた。動かなかったのは、すべて「バイナリがなく、ソースに落ちた」場合である。

そのため `pin_r()` の日付の候補は、表から引くのではなく、プロジェクトのパッケージごとにバイナリの有無を確かめて出す必要がある → **第6章の修正を提案する**。

## 提供範囲（月ごとの格子）

各月の最初のスナップショットで、その日付の版に HEAD を送った（User-Agent に R の版を入れる）。B はバイナリ、S はソースで、1文字が1か月（2017-10〜2026-10）を表す。

```
noble / data.table                                                          （2023-02〜）     （2024-02〜03 の穴）
  R 3.6: SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBBBBSSBBBBBBBBBBBBBBBBBBBBBSSSSSSSSSS
  R 4.4: SSSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
  R 4.5: SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBBBBBBBBBBBBBBBBBBB
  R 4.6: SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBB
noble / R6（R だけのパッケージ）
  R 3.6: SSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
  R 4.6: SSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBBBBBBBBBBBBB
noble / jsonlite
  R 4.4: SSSSSSSSSSSSSSSSSSSSSSSSSSSSBBBBBBBBBBBBBBBBBBBBBBBBBSSSSSSSSSSSSSSSSBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB
```

data.table で見た境界（日単位、`out/availability.json`）：

| | R 3.6 | R 4.0 | R 4.1 | R 4.2・4.3 | R 4.4 | R 4.5 | R 4.6 |
|---|---|---|---|---|---|---|---|
| noble | 2023-02-20〜2025-12-24 | 〜2025-05-12 | 〜2026-05-06 | 2023-02-20〜今日 | 2019-12-10〜今日 | 2024-08-28〜今日 | 2026-01-28〜今日 |
| jammy | 2019-12-10〜2025-12-24 | 〜2025-05-12 | 〜2026-05-06 | 2019-12-10〜今日 | 2019-12-10〜今日 | 2024-08-28〜今日 | 2026-01-28〜今日 |

読み取れること：

- **さかのぼってのビルド**：R 4.5（2025年4月公開）向けは 2024-08-28 から、R 4.6（2026年4月公開）向けは 2026-01-28 から存在した。noble（2024年4月公開）向けも、R 4.4 では 2019年までさかのぼる。
- **パッケージごとの違い**：R6 は、noble の R 3.6〜4.5 向けで 2019年12月から途切れない。jsonlite は、R 4.4・4.5 向けで 2022〜2023年に1年以上の穴がある。data.table は、古い R 向けが途中で打ち切られる（新しい版が古い R でビルドできないためと考えられる）。
- **R 4.0・4.1**：`/__api__/status` の `r_versions` に載っていないが、多くの日付でバイナリが返った。

## 今のディストリビューション（noble）での動作

まっさらな `ubuntu:24.04` に、R の実行時ライブラリ、libpcre3、sf の実行時ライブラリだけを入れた（コンパイラなし）。そのうえで一般ユーザーとして、data.table、jsonlite、R6、sf と、それらの依存を `install.packages()` で入れた。

| R @ 日付 | 対象 | バイナリ | 入らなかったもの | 読み込めたもの |
|---|---|---|---|---|
| 3.6.3 @ 2023-06-01 | 13 | 7 | sf、classInt、s2、units、e1071、wk | data.table、jsonlite、R6 |
| 4.2.3 @ 2023-06-01 | 13 | 7 | 同上 | 同上 |
| 4.4.3 @ 2020-06-01 | 10 | 7 | sf、classInt、e1071 | 同上 |
| 4.5.2 @ 2024-10-01（R の公開前） | 13 | 13 | なし | すべて（sf の GEOS・GDAL・PROJ も noble のものを使う） |
| 4.6.1 @ 2026-02-02（R の公開前） | 13 | 7 | sf、magrittr、s2、units、Rcpp | data.table、jsonlite、R6 |
| 4.6.1 @ 2026-10-01 | 12 | 12 | なし | すべて |

入らなかったパッケージは、HEAD で確かめると、すべて `x-package-type: source` だった（例：4.2.3 @ 2023-06-01 の sf 1.0-13、s2 1.1.4 など）。バイナリがありながら動かなかった例はない。

## 設計への影響

1. **pin_r の日付の候補（第6章）**：表で判定するのではなく、プロジェクトの解決済みのパッケージに、新しい R の版の User-Agent で HEAD を送って判定する。並列に送り、結果は永久にキャッシュする。候補は次の3つにする → **要件の修正を提案する**。
   - 今の日付のまま（バイナリのないパッケージはソースからのビルドになる。重い同期）
   - 全パッケージのバイナリが揃う、今の日付に最も近い日（前後どちらにもありうる）
   - 今日
2. **揃う日の探し方**：提供範囲には穴があるので、単純な二分探索の結果は確かめ直す必要がある。月ごとの粗い探索の後に日単位で詰め、最後に、その日の解決結果の全パッケージを HEAD で確かめる。
3. **renv からの移行（2-7）**：古いロックを新しいディストリビューションで再現すると、ソースからのビルドが多く出うる。重い同期として、どのパッケージがソースになるかを示す。
4. **バイナリの互換性**：届いたバイナリは今のディストリビューションで動いたので、ディストリビューションの不一致は考えなくてよい。P3M は、要求したディストリビューション向けにビルドしたものを返す。

## 追記：同じ版で日付を変えるとバイナリの有無は変わるか（2026-10-05、`per_version.py`）

pin_r の案として、「ロックの版を保ったまま、各パッケージがその版で最新だった期間の中から、新しい R 向けのバイナリがある日付を探す」方法が出た。その前提を確かめた。V4 の格子で穴や境界が見えた3か所の16の版について、その版が最新だった期間の中で日付を6つずつ変え、HEAD を送った。

| 対象 | 結果 |
|---|---|
| data.table / R 4.3 / noble（2024-02〜03 の穴） | 1.15.0 と 1.15.2 は、期間内のどの日付でも S。前後の版はどの日付でも B |
| jsonlite / R 4.4 / noble（2022〜2023 の穴） | 1.8.0〜1.8.5 は、どの日付でも S。1.7.x と 1.8.7 はどの日付でも B |
| data.table / R 4.6 / noble（さかのぼりの始まり） | 1.17.8 と 1.18.0 はどの日付でも S。1.18.2.1 はどの日付でも B |

**16の版のすべてで、期間内の日付によってバイナリの有無が変わることはなかった。** バイナリの有無は、（パッケージの版、R のマイナー版、ディストリビューション）で決まり、日付には依らない。

したがって、上の案は成り立たない。ロックの版にバイナリがなければ、その版が最新だった期間のどの日付に変えても、バイナリは見つからない。逆にバイナリがあれば、ロックに記録済みの日付のままで取得できる。

この性質から、次のことが言える。

- バイナリの有無の結果は、日付を除いたキーでキャッシュできる
- 探索の HEAD は、版ごとに1回で足りる

