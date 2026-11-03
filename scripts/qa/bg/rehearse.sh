#!/bin/bash
# rehearse.sh — G-final の録画 n（1..5）の BG の手を背景で通すリハーサル。台本は docs/demo/full.md（5 本とも同じ台本・n は回の番号と nonce の元）。
# 使い方:
#   bash scripts/qa/bg/rehearse.sh <n> --dry-run                 打つ手を印字して台本を検査するだけ（.app を起こさない・cargo 無し）
#   bash scripts/qa/bg/rehearse.sh <n> <Shibubu.app> [--out DIR]  静かな窓で。run.sh の front-guard のまま・起動の瞬間に HIDIdleTime を 1 回見る
# scene2 = 台本 docs/demo/scene2.md（複数の本物の app・band の一言 → 4 Page → REAPER の track・Discord の channel。27 手の full.md は変えない）。
# 出す物と保証は rehearse.py の冒頭。exit: 0 ok / 1 赤 / 2 使い方 / 3 起動できない（mirror-helper 無しの .app を含む） / 5 owner 使用中で未測 / 6 未測あり / 7 driver down
set -u
BG="$(cd "$(dirname "$0")" && pwd)"
case "${1:-}" in 1|2|3|4|5|scene2|short) ;; *) echo "usage: rehearse.sh <1..5|scene2|short> <Shibubu.app>|--dry-run" >&2; exit 2;; esac
n=$1; shift
[ "${1:-}" = "--dry-run" ] && exec python3 -I "$BG/rehearse.py" "$n" --dry-run
[ $# -ge 1 ] || { echo "usage: rehearse.sh <1..5|scene2|short> <Shibubu.app>|--dry-run" >&2; exit 2; }
exec python3 -I "$BG/rehearse.py" "$n" "$@"
