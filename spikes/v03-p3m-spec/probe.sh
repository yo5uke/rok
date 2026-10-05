#!/usr/bin/env bash
# V3: P3M の仕様を確かめる。取得物は out/ に置き、要点を標準出力に出す。
# 必要なもの：curl、jq、gawk、gzip、sha256sum、md5sum
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out

P3M=https://packagemanager.posit.co
CRAN=https://cloud.r-project.org
DATE=2026-10-01
OLD_DATE=2025-01-02
UA_44="R (4.4.2 x86_64-pc-linux-gnu x86_64 linux-gnu)"
UA_46="R (4.6.1 x86_64-pc-linux-gnu x86_64 linux-gnu)"
UA_46_ARM="R (4.6.1 aarch64-unknown-linux-gnu aarch64 linux-gnu)"

# PACKAGES の欄の名前と出現数
field_counts() {
  awk -F: '/^[A-Za-z]/{print $1}' "$1" | sort | uniq -c | sort -rn |
    awk '{printf "%s(%s) ", $2, $1}'
  echo
}
# PACKAGES から、あるパッケージの欄の値を取り出す（最初の記録のみ）
pkg_field() {
  awk -v p="$2" 'BEGIN{RS=""} $0 ~ "(^|\n)Package: "p"\n" {print; exit}' "$1" |
    awk -F': ' -v f="$3" '$1==f{print $2}'
}
# 応答ヘッダーから x-package-* だけを1行で出す
pkg_headers() {
  grep -iE '^x-package-(type|binary-tag):' "$1" | tr -d '\r' | tr '\n' ' '
  echo
}
fetch_index() { curl -fsS "$1" | gunzip > "$2"; }

echo "## 1. /__api__/status"
curl -fsS "$P3M/__api__/status" -o out/status.json
jq -r '"r_versions: \(.r_versions | join(", "))"' out/status.json
jq -r '.distros[] | select(.os == "linux" and .binaries and (.hidden | not))
  | "  \(.distribution) \(.release) -> \(.binaryURL) [\(.arch | join(","))]"' out/status.json
jq -c '.macos_urls' out/status.json

echo "## 2. スナップショットの日付（transaction-dates）"
curl -fsS "$P3M/__api__/repos/cran/transaction-dates" -o out/dates.json 2>/dev/null ||
  curl -fsS "$P3M/__api__/repos/2/transaction-dates" -o out/dates.json
jq -r '"count: \(length), first: \(.[0].alias), last: \(.[-1].alias)"' out/dates.json
for d in 2017-10-11 2017-01-01 2099-01-01; do
  printf '  %s -> ' "$d"
  curl -sS -o /dev/null -w '%{http_code}\n' "$P3M/cran/$d/src/contrib/PACKAGES.gz"
done

echo "## 3. ソースと Linux の索引"
fetch_index "$P3M/cran/$DATE/src/contrib/PACKAGES.gz" out/src-PACKAGES
fetch_index "$P3M/cran/__linux__/noble/$DATE/src/contrib/PACKAGES.gz" out/linux-PACKAGES
fetch_index "$CRAN/src/contrib/PACKAGES.gz" out/cran-PACKAGES
printf 'P3M source: '; field_counts out/src-PACKAGES
printf 'CRAN:       '; field_counts out/cran-PACKAGES
cmp -s out/src-PACKAGES out/linux-PACKAGES && echo "Linux index == source index"
echo "records with Path:"
awk 'BEGIN{RS=""} /\nPath:/' out/src-PACKAGES | grep -E '^(Package|Path):' | paste - - | head -3

echo "## 4. Linux のバイナリの判定（User-Agent と HEAD）"
DT_VER=$(pkg_field out/src-PACKAGES data.table Version)
url="$P3M/cran/__linux__/noble/$DATE/src/contrib/data.table_$DT_VER.tar.gz"
for ua in "curl/8" "$UA_44" "$UA_46" "$UA_46_ARM" "R (4.4 x86_64-pc-linux-gnu x86_64 linux-gnu)"; do
  printf '  %-52s ' "$ua"
  curl -sS -I -A "$ua" -o out/h.txt "$url"
  pkg_headers out/h.txt
done
echo "old snapshot ($OLD_DATE), data.table_1.16.4:"
for ua in "$UA_44" "$UA_46"; do
  printf '  %-52s ' "$ua"
  curl -sS -I -A "$ua" -o out/h.txt \
    "$P3M/cran/__linux__/noble/$OLD_DATE/src/contrib/data.table_1.16.4.tar.gz"
  pkg_headers out/h.txt
done
printf 'version not in snapshot -> '
curl -sS -o /dev/null -w '%{http_code}\n' \
  "$P3M/cran/__linux__/noble/$DATE/src/contrib/data.table_1.16.4.tar.gz"

echo "## 5. チェックサム"
api_sum=$(curl -fsS "$P3M/__api__/repos/cran/packages/data.table" | jq -r .checksum)
curl -fsSL -o out/dt-cran.tar.gz "$CRAN/src/contrib/data.table_$DT_VER.tar.gz"
curl -fsSL -A "curl/8" -D out/h-src.txt -o out/dt-p3m-src.tar.gz "$url"
curl -fsSL -A "$UA_46" -D out/h-bin.txt -o out/dt-p3m-bin.tar.gz "$url"
echo "  API checksum:            $api_sum"
echo "  sha256(CRAN source):     $(sha256sum out/dt-cran.tar.gz | cut -c1-64)"
echo "  sha256(P3M source):      $(sha256sum out/dt-p3m-src.tar.gz | cut -c1-64)"
echo "  md5(CRAN source):        $(md5sum out/dt-cran.tar.gz | cut -c1-32)  index MD5sum: $(pkg_field out/cran-PACKAGES data.table MD5sum)"
echo "  md5(P3M source/binary):  $(md5sum out/dt-p3m-src.tar.gz | cut -c1-32) / $(md5sum out/dt-p3m-bin.tar.gz | cut -c1-32)"
echo "  ETag (source/binary):    $(grep -i '^etag' out/h-src.txt | tail -1 | tr -d '\r"' | cut -d' ' -f2) / $(grep -i '^etag' out/h-bin.txt | tail -1 | tr -d '\r"' | cut -d' ' -f2)"
echo "  P3M source DESCRIPTION:  $(tar -xOzf out/dt-p3m-src.tar.gz data.table/DESCRIPTION | grep -E '^Repository:')"
echo "  P3M binary DESCRIPTION:  $(tar -xOzf out/dt-p3m-bin.tar.gz data.table/DESCRIPTION | grep -E '^Built:')"
printf '  API checksum of an archived version (1.16.4): '
curl -fsS "$P3M/__api__/repos/cran/packages/data.table?version=1.16.4" | jq -r '"\"\(.checksum)\""'

echo "## 6. Windows・macOS の索引"
for spec in "bin/windows/contrib/4.6|zip" "bin/macosx/sonoma-arm64/contrib/4.6|tgz"; do
  IFS='|' read -r path ext <<<"$spec"
  idx="out/$(echo "$path" | tr '/' '_')-PACKAGES"
  fetch_index "$P3M/cran/$DATE/$path/PACKAGES.gz" "$idx"
  echo "$path: $(grep -c '^Package:' "$idx") packages"
  printf '  fields: '; field_counts "$idx"
  ver=$(pkg_field "$idx" data.table Version)
  curl -fsSL -o "out/dt.$ext" "$P3M/cran/$DATE/$path/data.table_$ver.$ext"
  echo "  Hash: $(pkg_field "$idx" data.table Hash)  md5(file): $(md5sum "out/dt.$ext" | cut -c1-32)"
done

echo "## 7. システム依存（sysreqs）"
curl -fsS "$P3M/__api__/repos/cran/sysreqs?all=false&pkgname=sf&pkgname=data.table&distribution=ubuntu&release=24.04" |
  jq -r '.requirements[] | "  \(.name): \(.requirements.packages | join(" "))"'
