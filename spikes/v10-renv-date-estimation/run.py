#!/usr/bin/env python3
"""V10：「ある版が CRAN の最新だった期間」を P3M の情報から求め、renv.lock の日付を推定できるかを確かめる。

方法：P3M のパッケージ API（/__api__/repos/cran/packages/<pkg>）の現行版と archived の公開日時から、
版ごとの区間 [公開日時, 次の版の公開日時) を作る。スナップショットの日付 T に版 v が載るのは、
公開日時 < T（UTC の0時）≤ 次の版の公開日時 のときとみなす。

検証：過去のスナップショットの索引から30パッケージを抜き出して「ロック」とみなし、
(1) 各パッケージの区間に T が入るか、(2) 区間の共通部分（推定した日付の範囲）に T が入るか、を確かめる。
"""
import concurrent.futures as cf
import gzip
import json
import random
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

P3M = "https://packagemanager.posit.co"
OUT = Path(__file__).parent / "out"
API_CACHE = OUT / "api"
TARGETS = ["2018-06-01", "2020-03-02", "2022-09-01", "2024-06-03", "2025-11-03", "2026-09-01"]
SAMPLE = 30


def get(url):
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read()


def transaction_dates():
    f = OUT / "dates.json"
    if not f.exists():
        f.write_bytes(get(f"{P3M}/__api__/repos/cran/transaction-dates"))
    return sorted(d["alias"] for d in json.loads(f.read_text()))


def index(date):
    """スナップショットの索引を {パッケージ: 版} で返す（Path 付きの重複は除く）。"""
    f = OUT / f"idx-{date}.json"
    if f.exists():
        return json.loads(f.read_text())
    text = gzip.decompress(get(f"{P3M}/cran/{date}/src/contrib/PACKAGES.gz")).decode("utf-8", "replace")
    pkgs = {}
    for rec in text.split("\n\n"):
        fields = dict(line.split(": ", 1) for line in rec.splitlines() if ": " in line and not line.startswith(" "))
        if "Package" in fields and "Path" not in fields:
            pkgs[fields["Package"]] = fields["Version"]
    f.write_text(json.dumps(pkgs))
    return pkgs


def history(pkg):
    """版 → 公開日時（UTC、datetime）。公開日時のない版は除く。"""
    f = API_CACHE / f"{pkg}.json"
    if not f.exists():
        d = json.loads(get(f"{P3M}/__api__/repos/cran/packages/{pkg}"))
        slim = [{"version": d["version"], "date": d["date_publication"]}]
        slim += [{"version": a["version"], "date": a["date_publication"]} for a in d.get("archived") or []]
        f.write_text(json.dumps(slim))
    out = {}
    for e in json.loads(f.read_text()):
        if e["date"]:
            out[e["version"]] = datetime.fromisoformat(e["date"].replace("Z", "+00:00"))
    return out


def interval(hist, version):
    """版が最新だった区間 (公開日時, 次の版の公開日時 or None)。"""
    if version not in hist:
        return None
    start = hist[version]
    later = [t for t in hist.values() if t > start]
    return start, (min(later) if later else None)


def day(date):
    return datetime.fromisoformat(date).replace(tzinfo=timezone.utc)


def main():
    API_CACHE.mkdir(parents=True, exist_ok=True)
    dates = transaction_dates()
    rng = random.Random(42)
    total_ok = total = 0
    for target in TARGETS:
        t = max(d for d in dates if d <= target)  # 実在するスナップショットの日付に解決する
        idx = index(t)
        sample = sorted(rng.sample(sorted(idx), SAMPLE))
        start_clock = time.time()
        with cf.ThreadPoolExecutor(16) as ex:
            hists = dict(zip(sample, ex.map(history, sample)))
        elapsed = time.time() - start_clock

        ok, misses, unknown = 0, [], []
        lo, hi = None, None  # 推定する範囲：lo < T ≤ hi
        for pkg in sample:
            iv = interval(hists[pkg], idx[pkg])
            if iv is None:
                unknown.append(f"{pkg} {idx[pkg]}")
                continue
            start, end = iv
            if start < day(t) and (end is None or day(t) <= end):
                ok += 1
            else:
                misses.append(f"{pkg} {idx[pkg]} [{start:%Y-%m-%d %H:%M}, {end:%Y-%m-%d %H:%M}]" if end else f"{pkg} {idx[pkg]} [{start:%Y-%m-%d}, -]")
            lo = start if lo is None else max(lo, start)
            hi = end if hi is None or (end is not None and end < hi) else hi
        candidates = [d for d in dates if lo < day(d) and (hi is None or day(d) <= hi)]
        total_ok += ok
        total += SAMPLE - len(unknown)
        print(f"## {target} → snapshot {t}（API {SAMPLE} 件 {elapsed:.1f} 秒）")
        print(f"  区間に入った：{ok}/{SAMPLE - len(unknown)}、版の記録なし：{len(unknown)} {unknown}")
        for m in misses:
            print(f"  外れ：{m}")
        rng_txt = f"{candidates[0]} 〜 {candidates[-1]}（{len(candidates)} 日）" if candidates else "なし"
        print(f"  推定した範囲：{rng_txt}、正解を含む：{t in candidates}")
    print(f"## 合計：区間に入った {total_ok}/{total}")

    # 全版が揃う日がない場合：2024-06-03 の25件に、2023-06-01 の5件（更新されずに古いまま）を混ぜる
    new_t = max(d for d in dates if d <= "2024-06-03")
    old_t = max(d for d in dates if d <= "2023-06-01")
    new_idx, old_idx = index(new_t), index(old_t)
    stale = [p for p in sorted(old_idx) if p in new_idx and old_idx[p] != new_idx[p]]
    lock = {p: new_idx[p] for p in rng.sample(sorted(new_idx), 25)}
    lock.update({p: old_idx[p] for p in rng.sample(stale, 5)})
    with cf.ThreadPoolExecutor(16) as ex:
        hists = dict(zip(lock, ex.map(history, lock)))
    ivs = [interval(hists[p], v) for p, v in lock.items()]
    ivs = [iv for iv in ivs if iv]
    counts = {d: sum(1 for s, e in ivs if s < day(d) and (e is None or day(d) <= e)) for d in dates}
    best = max(counts.values())
    best_days = [d for d in dates if counts[d] == best]
    print(f"## 混在（{new_t} の25件＋{old_t} の5件）")
    print(f"  最も多く一致：{best}/{len(ivs)} 件、{best_days[0]} 〜 {best_days[-1]}（{len(best_days)} 日）、"
          f"{new_t} を含む：{new_t in best_days}")


if __name__ == "__main__":
    main()
