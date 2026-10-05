#!/usr/bin/env python3
"""V4（1b）：調べるパッケージを変えると提供範囲が変わるかを、noble の月ごとの格子で比べる（R6・jsonlite）。"""
import concurrent.futures as cf

import availability as a


def main():
    dates = a.v10.transaction_dates()
    months = {}
    for d in dates:
        months.setdefault(d[:7], d)
    grid = sorted(months.values())
    for probe in ["R6", "jsonlite"]:
        hist = a.v10.history(probe)
        a.PROBE = probe
        jobs = [(r, d) for r in a.R_VERSIONS for d in grid]

        def f(job, hist=hist):
            v = a.version_at(hist, job[1])
            return a.kind("noble", job[1], a.R_VERSIONS[job[0]], v) if v else "-"
        with cf.ThreadPoolExecutor(24) as ex:
            res = dict(zip(jobs, ex.map(f, jobs)))
        print(f"## noble / {probe}（{grid[0]} 〜 {grid[-1]}、1文字＝1か月）")
        for r in a.R_VERSIONS:
            print(f"  R {r}: " + "".join(res[(r, d)] for d in grid))


if __name__ == "__main__":
    main()
