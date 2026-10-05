#!/usr/bin/env python3
"""V4（3）：同じパッケージの同じ版で、その版が最新だった期間の中の日付を変えると、バイナリの有無が変わるかを確かめる。

pin_r の案「ロックの版を保ったまま、その版が最新だった期間の中から、新しい R 向けのバイナリがある日付を探す」
が意味を持つのは、同じ版でも日付によってバイナリの有無が変わる場合だけである。V4 の格子で穴や境界が見えた所で調べる。
"""
import concurrent.futures as cf
from datetime import datetime, timezone

import availability as a

CASES = [  # （パッケージ, R の版, 調べる期間）
    ("data.table", "4.3.3", "2023-12-01", "2024-05-31"),  # noble の R 4.3 で 2024-02〜03 に穴
    ("jsonlite", "4.4.3", "2022-01-01", "2023-08-31"),    # noble の R 4.4 で 2022-03〜2023-06 に穴
    ("data.table", "4.6.1", "2025-11-01", "2026-03-31"),  # R 4.6 向けのさかのぼりの始まり（2026-01-28）
]
SAMPLES = 6  # 版ごとに確かめる日付の数（期間の中で等間隔に取る）


def utc(d):
    return datetime.fromisoformat(d).replace(tzinfo=timezone.utc)


def main():
    dates = a.v10.transaction_dates()
    total = mixed = 0
    for pkg, r, lo, hi in CASES:
        hist = a.v10.history(pkg)
        print(f"## {pkg} / R {r} / noble（{lo} 〜 {hi} に最新だった版）")
        for v, pub in sorted(hist.items(), key=lambda kv: kv[1]):
            later = [t for t in hist.values() if t > pub]
            end = min(later) if later else None
            period = [d for d in dates if pub < utc(d) and (end is None or utc(d) <= end)]
            if not period or period[-1] < lo or period[0] > hi:
                continue
            step = max(1, len(period) // SAMPLES)
            picks = sorted(set(period[::step][:SAMPLES - 1] + [period[-1]]))
            a.PROBE = pkg
            with cf.ThreadPoolExecutor(8) as ex:
                kinds = list(ex.map(lambda d, v=v: a.kind("noble", d, r, v), picks))
            uniform = len(set(kinds)) == 1
            total += 1
            mixed += not uniform
            print(f"  {v:<10} {period[0]}〜{period[-1]}（{len(period)}日）: "
                  + " ".join(f"{d}={k}" for d, k in zip(picks, kinds)) + ("" if uniform else "  ← 日付で変わる"))
    print(f"## 版の数 {total}、日付でバイナリの有無が変わった版 {mixed}")


if __name__ == "__main__":
    main()
