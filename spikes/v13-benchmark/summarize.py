#!/usr/bin/env python3
"""V13：out/results.csv（または引数の CSV）を集計し、場面ごとの中央値・最小・最大・四分位範囲を Markdown の表で出す。
S1（start）は、基準（none：空のディレクトリ）の中央値との差を「上乗せ」として出す。
"""
import csv
import statistics as st
from collections import defaultdict
from pathlib import Path

import sys

# 引数で CSV を選べる（rok の測定は bench-rok.sh が out/results-rok.csv に書く）
src = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).parent / "out" / "results.csv"
rows = list(csv.DictReader(open(src)))
groups = defaultdict(list)
fails = defaultdict(int)
pkgs = {}
for r in rows:
    key = (r["scenario"], r["project"], r["tool"])
    if r["status"] != "0":
        fails[key] += 1
        continue
    groups[key].append(float(r["seconds"]))
    pkgs[key] = r["npkgs"]


def q(v):
    if len(v) < 4:
        return float("nan")
    qs = st.quantiles(v, n=4)
    return qs[2] - qs[0]


ORDER = ["cold", "warm", "solve", "start", "setup"]
TITLE = {"cold": "S4 キャッシュなし（cold）", "warm": "S3 キャッシュあり（warm）", "solve": "S2 依存の解決",
         "start": "S1 起動（変化なし）", "setup": "参考：ロックの作成（初回、測定前の準備）"}
for sc in ORDER:
    keys = sorted(k for k in groups if k[0] == sc)
    if not keys:
        continue
    print(f"\n### {TITLE[sc]}\n")
    extra = "| 上乗せ（中央値 − 基準）" if sc == "start" else ""
    print(f"| プロジェクト | ツール | n | 中央値 (s) | 最小 | 最大 | 四分位範囲 | パッケージ数 {extra}|")
    print("|---|---|---|---|---|---|---|---|" + ("---|" if sc == "start" else ""))
    for k in keys:
        v = groups[k]
        base = ""
        if sc == "start":
            b = groups.get(("start", k[1], "none"))
            base = f"| {st.median(v) - st.median(b):+.3f} " if b and k[2] != "none" else "| — "
        f = f"（失敗 {fails[k]}）" if fails[k] else ""
        print(f"| {k[1]} | {k[2]}{f} | {len(v)} | {st.median(v):.3f} | {min(v):.3f} | {max(v):.3f} | {q(v):.3f} | {pkgs[k]} {base}|")
for k, n in fails.items():
    if k not in groups:
        print(f"\n失敗のみ：{k} × {n}")
