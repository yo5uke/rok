#!/usr/bin/env python3
"""V4（1）：P3M が、R のマイナー版ごとに、どの日付のスナップショットで Linux バイナリを出しているかを調べる。

方法：各月の最初のスナップショットの日付で、data.table（その日付の版）に HEAD を送る。User-Agent に
R の版を入れ、x-package-type が binary かを見る。月の格子で境界を見つけた後、日単位で二分探索する。
版の決め方は V10 と同じ（P3M のパッケージ API の公開日時から、その日付で最新の版を求める）。
"""
import concurrent.futures as cf
import json
import sys
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent / "v10-renv-date-estimation"))
import run as v10  # noqa: E402  history()・interval()・transaction_dates() を使う

P3M = "https://packagemanager.posit.co"
OUT = Path(__file__).parent / "out"
R_VERSIONS = {"3.6": "3.6.3", "4.0": "4.0.5", "4.1": "4.1.3", "4.2": "4.2.3",
              "4.3": "4.3.3", "4.4": "4.4.3", "4.5": "4.5.2", "4.6": "4.6.1"}
DISTROS = ["noble", "jammy"]
PROBE = "data.table"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def version_at(hist, date):
    t = datetime.fromisoformat(date).replace(tzinfo=timezone.utc)
    cands = [(pub, v) for v, pub in hist.items() if pub < t]
    return max(cands)[1] if cands else None


def kind(distro, date, r_patch, version):
    """'B'＝バイナリ、'S'＝ソース、'-'＝取得できない。"""
    url = f"{P3M}/cran/__linux__/{distro}/{date}/src/contrib/{PROBE}_{version}.tar.gz"
    req = urllib.request.Request(url, method="HEAD",
                                 headers={"User-Agent": f"R ({r_patch} x86_64-pc-linux-gnu x86_64 linux-gnu)"})
    try:
        with OPENER.open(req, timeout=60) as r:
            t = r.headers.get("x-package-type", "")
    except urllib.error.HTTPError as e:
        if e.code not in (301, 302, 307, 308):
            return "-"
        t = e.headers.get("x-package-type", "")
    return "B" if t == "binary" else "S"


def main():
    OUT.mkdir(exist_ok=True)
    v10.API_CACHE.mkdir(parents=True, exist_ok=True)
    dates = v10.transaction_dates()
    hist = v10.history(PROBE)
    months = {}
    for d in dates:
        months.setdefault(d[:7], d)
    grid_dates = sorted(months.values())

    jobs = [(dist, r, d) for dist in DISTROS for r in R_VERSIONS for d in grid_dates]
    with cf.ThreadPoolExecutor(24) as ex:
        res = dict(zip(jobs, ex.map(lambda j: kind(j[0], j[2], R_VERSIONS[j[1]], version_at(hist, j[2])), jobs)))

    summary = {}
    print(f"月ごとの格子（{grid_dates[0]} 〜 {grid_dates[-1]}、1文字＝1か月。B＝バイナリ、S＝ソース）")
    for dist in DISTROS:
        print(f"## {dist}")
        for r in R_VERSIONS:
            line = "".join(res[(dist, r, d)] for d in grid_dates)
            print(f"  R {r}: {line}")
            b = [d for d in grid_dates if res[(dist, r, d)] == "B"]
            if not b:
                summary[f"{dist} {r}"] = None
                continue
            # 境界の二分探索：最初の B の月の前の格子点から、最後の B の月の次の格子点まで
            first_i, last_i = grid_dates.index(b[0]), grid_dates.index(b[-1])

            def probe(d, r=r, dist=dist):
                return kind(dist, d, R_VERSIONS[r], version_at(hist, d)) == "B"

            def bsearch(lo, hi, want_first, probe=probe):
                # dates[lo..hi] の中で、B と S の境目を探す
                while hi - lo > 1:
                    mid = (lo + hi) // 2
                    if probe(dates[mid]) == want_first:
                        hi = mid
                    else:
                        lo = mid
                return hi
            lo = dates.index(grid_dates[first_i - 1]) if first_i > 0 else 0
            first = dates[bsearch(lo, dates.index(b[0]), True)] if first_i > 0 else dates[0]
            hi = dates.index(grid_dates[last_i + 1]) if last_i + 1 < len(grid_dates) else len(dates) - 1
            if last_i + 1 < len(grid_dates):
                last = dates[bsearch(dates.index(b[-1]), hi, False) - 1]
            else:
                last = dates[-1]
            gaps = line[line.index("B"):line.rindex("B") + 1].count("S")
            summary[f"{dist} {r}"] = {"first": first, "last": last, "gap_months": gaps}
    print("## 境界（日単位）")
    for k, v in summary.items():
        print(f"  {k}: " + (f"{v['first']} 〜 {v['last']}（範囲内でソースだった月：{v['gap_months']}）" if v else "バイナリなし"))
    (OUT / "availability.json").write_text(json.dumps(summary, indent=1))


if __name__ == "__main__":
    main()
