#!/bin/bash
# record.sh — G-final の録画 mode（DEMO-5）。owner が机に居る前で、台本の手を既存の driver（rehearse.py）で打ちながら画面を録る。台本 = --script short（docs/demo/short.md の 9 手・録画する物・既定）| full（docs/demo/full.md の 27 手・回帰の試験）| scene2（docs/demo/scene2.md・2 本目のテイク）。
# 使い方:
#   bash scripts/qa/bg/record.sh <Shibubu.app> [--script short|full|scene2] --take N [--out DIR] [--from 手番] [--dry-run]
#   --dry-run  手の表（driver / owner / 未測 の印）と env を印字するだけ。.app も screencapture も起こさない・cargo 無し
# 録画 mode = SHIBUBU_QA_RECORD=1（run.sh と rehearse.py が読む）: HIDIdleTime は記録だけ・open は前面・手ごとの写真と front-guard の judge は飛ばす（run 全体の judge だけ取る）。製品の code は触らない。
# 出す物（<out> = 既定 $HOME/shibubu-yc-runs/rec-take-N）: take-N.mov（screencapture -v）・take-N.chapters.tsv（手ごとの録画の始めからの秒）・run-take-N/（run の dir）。
# owner の手: 台本の FG の手（full = 6・8・17・19・24 / short = 6・8 / 番号は --dry-run の表の [owner]）は「[owner] …」を出して Enter を待つ（待つ間も録画は続く）。今 driver の無い手は無い（rehearse.py の NO_DRIVER = {}）。打たない手は条件つきで未測 / blocked として章に残す: b4（GFINAL_REALLY=1 が無い）・#18 の ask_ai（人の lease が要る）・前提の手が ok でない手（rehearse.py の REQUIRES: #10←9・#22←5・#23←22・#26←25・#27←25 は blocked(#N)）。
# 本物の AI が既定（104 通目）: SHIBUBU_QA_LLM_OK=<owner の語> が無ければ rehearse.py が exit 8（何も起こさない）。偽の CLI は REHEARSE_LLM=fake と書いた時だけで、その run は run.json と報告の頭の行に `AI: fake`。上限は台本ごと（rehearse.py の LLM_BUDGET）。
# exit: rehearse.py と同じ（0 ok / 1 赤 / 2 使い方 / 3 起動できない / 6 未測あり）。6 は load1 が 10 を超えていて起こさなかった時も
set -u
BG="$(cd "$(dirname "$0")" && pwd)"
usage() { echo "usage: record.sh <Shibubu.app> [--script short|full|scene2] --take N [--out DIR] [--from 手番] [--dry-run]" >&2; exit 2; }
app="" take="" out="" from="" script=short dry=0  # 既定は録画する short（REC-SHORT: owner が record.md のとおり --script 無しで打つと 27 手が走っていた）
while [ $# -gt 0 ]; do case $1 in
  --take) take=${2:-}; shift 2 || usage ;; --out) out=${2:-}; shift 2 || usage ;; --from) from=${2:-}; shift 2 || usage ;;
  --script) script=${2:-}; shift 2 || usage ;; --dry-run) dry=1; shift ;; -*) usage ;; *) [ -z "$app" ] && app=$1 || usage; shift ;; esac; done
case $take in 1|2|3|4|5) ;; *) usage ;; esac
case $script in full) sel=$take ;; short|scene2) sel=$script ;; *) usage ;; esac  # rehearse.py の第 1 引数: full は take 番号（nonce に入る）・他は台本の名前
case ${from:-1} in ""|*[!0-9]*) usage ;; esac
out=${out:-$HOME/shibubu-yc-runs/rec-take-${take}}
mov="${out}/take-${take}.mov"; ch="${out}/take-${take}.chapters.tsv"
# 録画の take だけ AI の呼びの上限を 10（製品の既定）にする: REHEARSE_LLM_CALLS が空の時だけ 10 を渡す（短い台本の成功の道は 8 回ちょうどで余りが 0・cu.vKn8 / cu.GkMp）。外から渡せばそちら。rehearse.py の既定 8 は試験の上限のまま
REC_LLM_CALLS=${REHEARSE_LLM_CALLS:-10}

if [ $dry = 1 ]; then
  tbl=$(python3 -I "$BG/rehearse.py" "$sel" --dry-run) || { echo "$tbl"; exit 1; }
  echo "$tbl" | sed -e 's/\[driver 在り\]/[driver]/' -e 's/\[FG: 通さない\]/[owner]/' -e 's/\[driver 無し:.*\]/[未測]/'
  echo "録画 mode: SHIBUBU_QA_RECORD=1 · 台本=${script} · SHIBUBU_QA_FROM=${from:-1} · AI の上限 ${REC_LLM_CALLS} 回 · app=${app:-未指定} · 録画=${mov} · 章=${ch}"
  echo "owner の手 $(echo "$tbl" | grep -c '\[FG: 通さない\]') · 未測（driver 無し）$(echo "$tbl" | grep -c '\[driver 無し')"
  exit 0
fi
[ -n "$app" ] && [ -d "$app" ] || usage
# B77: 混んでいると screencapture が録り始めず（load1 118〜245 の測りでは 2 秒の録画が 15 秒たっても 1 コマも書かない）、章の 0（下の sleep 1）がずれる。load1 が 10 を超えていたら何も起こさない（--dry-run は門の外）
load1=$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}'); if awk -v l="${load1:-0}" 'BEGIN { exit !(l + 0 > 10) }'; then echo "機械が混んでいます（load ${load1}）。下がってから打ってください"; exit 6; fi
mkdir -p "${out}/run-take-${take}" || exit 3
# B77: screencapture -v は stdin の 1 文字か EOF で録画を止める（getchar・「type any character (or ctrl-c) to stop」）。& の子の stdin は既定で /dev/null なので、起きた直後に 1 コマ（0.03 秒）だけ録って終わっていた。
# 書く口（fd 3）を record.sh が持つ間は止まらない。止めるのは従来どおり kill -INT（本物で 1.1 秒）。record.sh が落ちても口が閉じて EOF で止まる（rehearse.py には 3>&- で渡さない）
scin="${out}/.sc-stdin.$$"; rm -f "$scin"; mkfifo "$scin" || exit 3
screencapture -v "$mov" <"$scin" & sc=$!
exec 3>"$scin"; rm -f "$scin"
trap 'kill -INT "$sc" 2>/dev/null' EXIT
sleep 1  # screencapture が録り始めるまで（この後の秒が章の 0）
T0=$(python3 -I -c 'import time;print(time.time())')
SHIBUBU_QA_RECORD=1 SHIBUBU_QA_REC_T0="$T0" SHIBUBU_QA_CHAPTERS="$ch" SHIBUBU_QA_FROM="${from:-1}" REHEARSE_LLM_CALLS="$REC_LLM_CALLS" python3 -I "$BG/rehearse.py" "$sel" "$app" --out "${out}/run-take-${take}" 3>&-
rc=$?
kill -INT "$sc" 2>/dev/null; wait "$sc" 2>/dev/null; trap - EXIT; exec 3>&-
echo "録画: ${mov}（$(stat -f %z "$mov" 2>/dev/null || echo 0) bytes）· 章: ${ch} · run: ${out}/run-take-${take}"
if [ -f "$ch" ]; then
  echo "owner の手 $(awk -F'\t' '$6 == "owner"' "$ch" | wc -l | tr -d ' ') · 未測 $(awk -F'\t' '$6 ~ /^未測/ {print $1 ":" $3}' "$ch" | tr '\n' ' ')· 録画の長さ $(awk -F'\t' 'END {print $5}' "$ch") 秒"
fi
exit $rc
