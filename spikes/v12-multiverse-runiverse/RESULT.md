# V12：R-multiverse・r-universe の仕様

実施：2026-10-05
再現：`./probe.sh`

## 結論

**成功。** URL の形式とバイナリの提供範囲が分かった。

- R-multiverse の Production には、日付つきのスナップショットがある（四半期ごと）。
- r-universe（R-multiverse の Community を含む）は、最新の版だけを置く。
- **どちらも、Ubuntu 24.04（noble）では、コンパイルが要るパッケージはソースからのビルドになる。**
- 索引には SHA256 が載っていて、ダウンロード時に照合できる。

## R-multiverse

| | Community | Production |
|---|---|---|
| URL | `https://community.r-multiverse.org` | `https://production.r-multiverse.org/<YYYY-MM-DD>` |
| 中身 | 登録されたパッケージの最新のリリース（113件） | 品質の確認を経た部分集合（2026-09-15 の分は78件） |
| 版の固定 | なし（最新のみ） | スナップショットごとに固定。公開後は変わらない |
| 日付 | — | 年4回（3・6・9・12月の15日）。各スナップショットには、CRAN の依存を固定した日（dependency freeze）があり、P3M のその日付と組み合わせて使うよう案内されている（例：2026-09-15 には `https://packagemanager.posit.co/cran/2026-07-15`） |
| Linux のバイナリ | r-universe と同じ（下記） | **なし**（ソース、Windows、macOS のみ） |
| 古い分 | — | 1年を過ぎたスナップショットは、バイナリが消える。ソースは残る（2025-03-15 の分は、ソースが 200、Windows 4.4 が 404） |

- 過去のスナップショットの一覧は、https://r-multiverse.org/production.html の Archive の表にある（機械向けの API は見当たらなかった）。
- Production の索引は、`SHA256` の値を DCF の継続行に書いている（`SHA256:` の次の行に、字下げして値がある）。この SHA256 は、ファイルの実体と一致した。

## r-universe

| 項目 | 形式 |
|---|---|
| ソース | `https://<owner>.r-universe.dev/src/contrib/PACKAGES.gz` |
| Linux | `.../bin/linux/<distro>-<arch>/<R のマイナー版>/src/contrib/PACKAGES.gz`（例：`noble-x86_64/4.6`、`resolute-x86_64/4.6`、`noble-aarch64/4.6`） |
| Windows | `.../bin/windows/contrib/<x.y>/PACKAGES.gz` |
| パッケージの情報 | `https://<owner>.r-universe.dev/api/packages/<pkg>`（`_binaries` に、OS・ディストリビューション・arch・R の版ごとのビルドの結果がある） |

- **Linux のバイナリ**：索引に `Built` 欄がある項目がバイナリで、ない項目は同じ URL でソースが返る（どの User-Agent でも同じ）。2026-10-05 時点の件数は次のとおり。
  - ropensci、R 4.6、resolute：コンパイルが要るもの 57/57 がバイナリ
  - ropensci、R 4.6、noble：コンパイルが要るもの 2/57 がバイナリ
  - R だけのパッケージ：どのディストリビューションでもバイナリ
  - R 4.5 の Linux の索引：`Built` が1件もない

  API で確かめると、magick の Linux 向けのビルドは resolute（Ubuntu 26.04）の R 4.6.1 と 4.7.0 だけだった。**コンパイルが要るパッケージの Linux バイナリは、最新の Ubuntu LTS の、R の現行版と開発版向けだけ**と判断できる。
- **ファイルの置き場所**：索引の `Path` は `<file>?sha256=<hash>&file=` の形をしている。ダウンロードは、SHA256 をキーにした保存先（例：`r2.ropensci.org/<sha256>`）への 302 になる。
- **チェックサム**：ソースにも、バイナリにも `SHA256` と `Filesize` がある。ソース（fluidsynth）とバイナリ（californiaalw）の両方で、実体と一致した。
- **出どころ**：DESCRIPTION に `RemoteUrl`（GitHub のリポジトリ）と `RemoteSha`（コミット）が入っている。
- **過去の版**：置かれるのは最新の版だけである。

## 設計への影響

1. **Production を日付つきのリポジトリとして扱う（第7章）**：`[repositories]` に Production を書けば、その日付で版が決まる。P3M と同じく、ロックに日付を記録できる。CRAN の依存は、案内されている P3M の日付と組み合わせると、確認済みの組み合わせになる。
2. **Linux では、多くがソースからのビルドになる**：noble では、Production にも Community にも、コンパイルが要るパッケージのバイナリがない。重い同期として扱い、ビルドの結果はグローバルキャッシュに残す。
3. **ダウンロード時の照合（第9章）**：索引の SHA256 と照合できる。P3M（Linux）より強い保証になる。
4. **ロックのハッシュ（第7章）**：CRAN 以外の CRAN 形式のリポジトリでは、索引にあるソースの SHA256 を、そのまま記録できる → **要件の追記を提案する**。
5. **版が消えたときの備え**：r-universe や Community は過去の版を残さない。要件どおり、取得済みのファイルをキャッシュに残す。さらに、DESCRIPTION の `RemoteSha` をロックに記録しておけば、キャッシュがない環境でも、GitHub から同じコミットを取得してビルドし直せる（選択肢として提案する）。
6. **索引の解析**：DCF の継続行に対応する（Production の SHA256）。
