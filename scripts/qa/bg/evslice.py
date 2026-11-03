#!/usr/bin/env python3
"""evslice.py <events.jsonl> <lo> — judge に渡す events の切り出し（W15-b59）。先頭の start の行 + t >= lo-1 の行を、後ろから読んで標準出力へ。
events.jsonl は t 順の追記なので、全部読む（run が進むほど 15MB・手ごとに 13 秒）代わりに末尾から lo-1 を越えるまで読む。
judge が見るのは [lo, hi] の行と「start が lo 以前・hi 以後の行が在る・hi より前に stop が無い」だけなので、切り出しても verdict は変わらない（test.sh が全 run で突き合わせる）。
"""
import re, sys
T = re.compile(rb'"t": ?([0-9.]+)')
path, lo = sys.argv[1], float(sys.argv[2]) - 1
try:
    f = open(path, "rb")
except OSError:
    sys.exit(0)  # 無い file は空のまま（judge が watcher_not_running にする）
with f:
    first = f.readline()
    size = f.seek(0, 2)
    n = 1 << 16
    while True:
        n = min(n, size)
        f.seek(size - n)
        lines = f.read(n).split(b"\n")
        if n < size:
            lines = lines[1:]  # 先頭は途中で切れている
        ts = [m and float(m.group(1)) for m in map(T.search, lines[:1])]
        if n >= size or (ts and ts[0] is not None and ts[0] < lo):
            break
        n *= 4
sys.stdout.buffer.write(first)
for l in lines:
    m = T.search(l)
    if m and float(m.group(1)) >= lo and l != first.rstrip(b"\n"):
        sys.stdout.buffer.write(l + b"\n")
