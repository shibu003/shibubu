#!/bin/bash
# retest-red.sh — 門の赤を「実物 / 環境由来 / 壊れ」に機械で分ける（cargo test の log を読む。cargo は 1 本ずつ直列・待たない）。
# 使い方: bash scripts/qa/retest-red.sh <cargo test の log> [--times 2] [--json]
#   log の `test <name> ... FAILED` を binary ごとに拾い（`Running tests/<bin>.rs` → --test <bin> / `Running unittests src/main.rs` → --bin shibubu /
#   src/lib.rs → --lib。Doc-tests は除く）、1 本ずつ CARGO_BUILD_JOBS=1 cargo test <選択> <name> -- --exact --nocapture を --times 回回す（各回の前の load1 を表に書く・判断は読む側）。
#   全回緑 = 環境由来 / 1 回でも赤 = 実物 / compile 不能か 1 本も走らない = 壊れ。
# exit: 0 実物も壊れも 0（赤 0 を含む）/ 1 実物か壊れあり / 2 使い方・log が読めない
set -u
exec python3 -I - "$@" <<'PY'
import json, re, subprocess, sys, os

args = sys.argv[1:]
times, as_json, log = 2, False, None
i = 0
while i < len(args):
    a = args[i]
    if a == "--json": as_json = True
    elif a == "--times" and i + 1 < len(args) and args[i + 1].isdigit() and int(args[i + 1]) > 0: times = int(args[i + 1]); i += 1
    elif log is None and not a.startswith("--"): log = a
    else: print("使い方: retest-red.sh <log> [--times N] [--json]", file=sys.stderr); sys.exit(2)
    i += 1
if log is None: print("使い方: retest-red.sh <log> [--times N] [--json]", file=sys.stderr); sys.exit(2)
try: text = open(log, encoding="utf-8", errors="replace").read()
except OSError as e: print(f"retest-red: log が読めない: {e}", file=sys.stderr); sys.exit(2)

# log → (binary の選択, test 名) 。binary の見出しは各 test より前に出る
sel, reds = None, []
for line in text.splitlines():
    m = re.match(r"\s*Running (tests/(\S+)\.rs|unittests (\S+))", line)
    if m:
        if m.group(2): sel = ["--test", m.group(2)]
        elif m.group(3) == "src/main.rs": sel = ["--bin", "shibubu"]
        elif m.group(3) == "src/lib.rs": sel = ["--lib"]
        else: sel = None
        continue
    if line.lstrip().startswith("Doc-tests"): sel = None; continue
    m = re.match(r"test (\S+) \.\.\. FAILED", line)
    if m and sel and (sel, m.group(1)) not in [(s, n) for s, n in reds]: reds.append((sel, m.group(1)))

def load1():
    try: return subprocess.run(["sysctl", "-n", "vm.loadavg"], capture_output=True, text=True).stdout.split()[1]
    except Exception: return "-"

def once(sel, name):
    l = load1()
    p = subprocess.run(["cargo", "test", *sel, name, "--", "--exact", "--nocapture"], capture_output=True, text=True,
                       env={**os.environ, "CARGO_BUILD_JOBS": "1"})
    out = p.stdout + p.stderr
    m = re.search(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed", out)
    if not m or (m.group(2) == "0" and m.group(3) == "0"): return l, "壊れ"   # compile 不能・名前が合わず 0 本
    return l, "緑" if m.group(1) == "ok" and p.returncode == 0 else "赤"

rows = []
for sel, name in reds:
    rs = [once(sel, name) for _ in range(times)]
    res = [r for _, r in rs]
    verdict = "壊れ" if "壊れ" in res else "実物" if "赤" in res else "環境由来"
    rows.append({"test": name, "binary": " ".join(sel), "runs": res, "load1": [l for l, _ in rs], "verdict": verdict})
n = {v: sum(r["verdict"] == v for r in rows) for v in ("実物", "環境由来", "壊れ")}
summary = f"赤 {len(rows)} 本: 実物 {n['実物']} / 環境由来 {n['環境由来']} / 壊れ {n['壊れ']}"
if as_json: print(json.dumps({"rows": rows, "summary": summary, **n}, ensure_ascii=False, indent=1))
else:
    print("| test | binary | 単独の結果 | load1 | 判定 |\n|---|---|---|---|---|")
    for r in rows: print(f"| {r['test']} | {r['binary']} | {' / '.join(r['runs'])} | {' / '.join(r['load1'])} | {r['verdict']} |")
    print(summary)
sys.exit(1 if n["実物"] or n["壊れ"] else 0)
PY
