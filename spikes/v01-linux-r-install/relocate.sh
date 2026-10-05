#!/bin/sh
# Posit の R ビルド（tar.gz）を任意の場所に展開し、動くように書き換える。
# 使い方：relocate.sh <R-x.y.z-<os>.tar.gz> <置き場の親ディレクトリ>
# 結果：<親>/<x.y.z>/bin/R、<親>/<x.y.z>/bin/Rscript
# 管理者権限は使わない。POSIX sh、tar、sed だけで動く（まっさらな Ubuntu でも動かすため）。
set -eu

tarball=$1
root=$2

# tar.gz の最上位のディレクトリが版になっている（例：4.4.2/）
ver=$(tar -tzf "$tarball" | head -1 | cut -d/ -f1)
mkdir -p "$root"
root=$(cd "$root" && pwd)
prefix="$root/$ver"
rm -rf "$prefix"
tar -xzf "$tarball" -C "$root"

# 1. テキストのファイルに埋め込まれた /opt/R/<版> を書き換える
#    （bin/R と lib/R/bin/R は同じシェルスクリプト。Makeconf はコメント行のみ）
for f in bin/R lib/R/bin/R lib/pkgconfig/libR.pc lib/R/etc/Makeconf; do
  sed -i "s|/opt/R/$ver|$prefix|g" "$prefix/$f"
done

# 2. Rscript はバイナリに R_HOME が埋め込まれていて書き換えられない。
#    環境変数 RHOME を渡すラッパーに置き換える。元のバイナリは bin/exec/ に置かない
#    （R CMD INSTALL が bin/exec/ の中身をサブアーキテクチャとみなすため）。
mv "$prefix/lib/R/bin/Rscript" "$prefix/lib/R/bin/Rscript.orig"
rm "$prefix/bin/Rscript"
for f in "$prefix/bin/Rscript" "$prefix/lib/R/bin/Rscript"; do
  cat > "$f" <<EOF
#!/bin/sh
# rok: the Rscript binary has its original R_HOME compiled in, so pass RHOME.
RHOME="$prefix/lib/R" exec "$prefix/lib/R/bin/Rscript.orig" "\$@"
EOF
  chmod +x "$f"
done

# 3. 書き換え漏れがないかを確かめる（Rscript.orig だけは残ってよい）
left=$(grep -rl "/opt/R/$ver" "$prefix" | grep -v 'Rscript.orig$' || true)
if [ -n "$left" ]; then
  echo "unexpected /opt/R references:" >&2
  echo "$left" >&2
  exit 1
fi
echo "$prefix"
