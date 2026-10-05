#!/usr/bin/env bash
# V3b（補足）：apt-cache depends が apt のリスト（/var/lib/apt/lists）を必要とするかを、docker の最小イメージで確かめる。
set -euo pipefail

echo "## ubuntu:24.04（リストなし、一般ユーザー）"
docker run --rm --user "$(id -u):$(id -g)" ubuntu:24.04 sh -c '
  echo "lists: $(ls /var/lib/apt/lists | grep -vc -e lock -e partial) files"
  apt-cache depends libgdal-dev >/dev/null 2>&1; echo "apt-cache depends libgdal-dev: exit=$?"
  echo "apt-cache policy libgdal34t64: [$(apt-cache policy libgdal34t64)] exit=$?"
'

echo "## ubuntu:24.04（root で apt-get update の後、一般ユーザー）"
docker run --rm ubuntu:24.04 sh -c "
  apt-get update -qq >/dev/null 2>&1
  echo \"lists: \$(du -sh /var/lib/apt/lists | cut -f1)\"
  setpriv --reuid=$(id -u) --regid=$(id -g) --clear-groups sh -c '
    apt-cache depends libgdal-dev | grep -m1 Depends; echo \"exit=\$?\"'
"

echo "## rocker/geospatial（リストを消したイメージ。インストール済みと未インストールの比較）"
docker run --rm --entrypoint sh rocker/geospatial:4.6.1 -c '
  echo "lists: $(ls /var/lib/apt/lists | grep -vc -e lock -e partial) files"
  for p in libgdal-dev libjags-dev; do
    dpkg -s "$p" >/dev/null 2>&1 && s=installed || s=not-installed
    apt-cache depends "$p" >/dev/null 2>&1; echo "$p ($s): exit=$?"
  done
'
