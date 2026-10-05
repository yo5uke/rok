#!/usr/bin/env bash
# V12：R-multiverse（Production・Community）と r-universe の URL の形式、Linux バイナリの有無、SHA256 を確かめる。
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p out

PROD=https://production.r-multiverse.org/2026-09-15
COMM=https://community.r-multiverse.org
RO=https://ropensci.r-universe.dev

count() { gunzip -c "$1" | grep -c '^Package:'; }

echo "## Production（$PROD）"
for path in src/contrib bin/windows/contrib/4.6 bin/macosx/sonoma-arm64/contrib/4.6 bin/linux/noble-x86_64/4.6/src/contrib; do
  code=$(curl -sS -o out/p.gz -w '%{http_code}' "$PROD/$path/PACKAGES.gz")
  if [ "$code" = 200 ]; then echo "  $path: $(count out/p.gz) packages"; else echo "  $path: HTTP $code"; fi
done
printf '  1年を過ぎた分（2025-03-15）：src %s、windows 4.4 %s\n' \
  "$(curl -sS -o /dev/null -w '%{http_code}' https://production.r-multiverse.org/2025-03-15/src/contrib/PACKAGES.gz)" \
  "$(curl -sS -o /dev/null -w '%{http_code}' https://production.r-multiverse.org/2025-03-15/bin/windows/contrib/4.4/PACKAGES.gz)"
# SHA256 は DCF の継続行に書かれている
curl -fsS "$PROD/src/contrib/PACKAGES.gz" | gunzip > out/prod-src.txt
ver=$(awk 'BEGIN{RS=""} /(^|\n)Package: polars\n/' out/prod-src.txt | awk -F': ' '/^Version/{print $2}')
sha=$(awk 'BEGIN{RS=""} /(^|\n)Package: polars\n/' out/prod-src.txt | awk '/^SHA256:/{getline; print $1}')
curl -fsSL -o out/polars.tar.gz "$PROD/src/contrib/polars_$ver.tar.gz"
echo "  polars $ver: index ${sha:0:16}… file $(sha256sum out/polars.tar.gz | cut -c1-16)…"

echo "## Linux の索引：コンパイルが要るものと、バイナリ（Built 欄あり）の数"
for base in "$COMM" "$RO"; do
  for d in noble-x86_64/4.6 resolute-x86_64/4.6 noble-x86_64/4.5; do
    curl -fsS "$base/bin/linux/$d/src/contrib/PACKAGES.gz" | gunzip |
      awk -v t="$base $d" 'BEGIN{RS=""} {c=/NeedsCompilation: yes/; b=/\nBuilt:/; n[c" "b]++}
        END{printf "  %s: compiled %d (binary %d), pure R %d (binary %d)\n", t, n["1 1"]+n["1 0"], n["1 1"], n["0 1"]+n["0 0"], n["0 1"]}'
  done
done

echo "## r-universe の API（magick の Linux 向けビルド）"
curl -fsS "$RO/api/packages/magick" | jq -r '._binaries[] | select(.os == "linux") | "  \(.distro) \(.arch) R \(.r) \(.status)"'

echo "## r-universe の SHA256（R だけのパッケージのバイナリ）"
curl -fsS "$COMM/bin/linux/noble-x86_64/4.6/src/contrib/PACKAGES.gz" | gunzip > out/comm-noble.txt
rec=$(awk 'BEGIN{RS=""} /\nBuilt:/ {print; exit}' out/comm-noble.txt)
pkg=$(echo "$rec" | awk -F': ' '/^Package/{print $2}')
ver=$(echo "$rec" | awk -F': ' '/^Version/{print $2}')
sha=$(echo "$rec" | awk -F': ' '/^SHA256/{print $2}')
path=$(echo "$rec" | awk -F': ' '/^Path/{print $2}')
curl -fsSL -o out/bin.tar.gz "$COMM/bin/linux/noble-x86_64/4.6/src/contrib/$path/${pkg}_$ver.tar.gz"
echo "  $pkg $ver: index ${sha:0:16}… file $(sha256sum out/bin.tar.gz | cut -c1-16)…"
tar -xOzf out/bin.tar.gz "$pkg/DESCRIPTION" | grep -E '^(Built|RemoteUrl|RemoteSha):' | sed 's/^/  /'
