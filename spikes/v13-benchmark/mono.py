#!/usr/bin/env python3
"""コマンドを実行し、かかった時間を単調時計で測って「秒 終了コード」を出力する。
WSL2 では壁時計（date）が測定中に飛ぶことがあったため（pak の cold が 0.2 秒、pak 自身の表示が -100ms になった）。
使い方：mono.py <ログのパス> <コマンド...>。コマンドは bash -c で実行するので、export -f した関数も使える。
"""
import subprocess
import sys
import time

log, cmd = sys.argv[1], sys.argv[2:]
with open(log, "w") as f:
    start = time.monotonic_ns()
    r = subprocess.run(["bash", "-c", '"$@"', "bench"] + cmd, stdout=f, stderr=subprocess.STDOUT)
    elapsed = time.monotonic_ns() - start
print(f"{elapsed / 1e9:.3f} {r.returncode}")
