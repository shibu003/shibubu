#!/bin/bash
# quiet-window.sh — 静かな窓を 1 command で流す。表（TSV: name<TAB>cwd<TAB>command・# は註・$HOME だけ展開）の行を直列に流し、1 つの表に残す。
# 使い方:
#   bash scripts/qa/bg/quiet-window.sh --dry-run <plan.tsv>             表の行と今の門の値（cargo・idle・load1）を印字・何も起こさない
#   bash scripts/qa/bg/quiet-window.sh --check <plan.tsv>               窓を開ける前の前検査: 行ごとに cwd・.app・driver の file・driver の --dry-run（.app を起こさない）・道具（cwd の scripts/qa/bg/bin/{axprobe,front-guard,windows,wincap} が実行可能か・無い物は ng(名)）を ok / ng で表にする（exit 0 全部 ok / 1 ng あり）
#   bash scripts/qa/bg/quiet-window.sh <plan.tsv> [--out DIR]           各行の前に cargo 本数と owner の idle（owner_idle.py）を 1 回だけ読む（待たない）
# 門は 3 つ（この順）: cargo/rustc の本数 > QW_CARGO_MAX（既定 0）→ idle < QW_IDLE_MIN（既定 120 秒）→ load1 > QW_LOAD1_MAX（既定 = 論理 CPU 数）。
# no-go ならその行と残りを unmeasured（理由つき）にして exit 6。command の exit 6（driver の owner_returned）は unmeasured (owner)・exit 7（同じ driver error が 3 手続いた）は unmeasured (driver: 原文の 1 行)・以降の行は続ける。command が空か # 始まりの行は skip。
# 検査用の差し替え: QW_CARGO_N / QW_IDLE_S / QW_LOAD1。
# exit: 0 全部 ok / 1 fail あり / 2 使い方・表が壊れている / 6 未測あり
set -u
usage() { echo "usage: quiet-window.sh --dry-run <plan.tsv> | --check <plan.tsv> | <plan.tsv> [--out DIR]" >&2; exit 2; }
DRY=0; CHK=0; [ "${1:-}" = "--dry-run" ] && { DRY=1; shift; }; [ "${1:-}" = "--check" ] && { CHK=1; shift; }
[ $# -ge 1 ] || usage
PLAN=$1; shift; OUT=""
while [ $# -gt 0 ]; do case $1 in --out) [ $# -ge 2 ] || usage; OUT=$2; shift 2;; *) usage;; esac; done
[ -f "$PLAN" ] || { echo "quiet-window: ${PLAN} が無い" >&2; exit 2; }
MIN=${QW_IDLE_MIN:-120}; MAXC=${QW_CARGO_MAX:-0}; MAXL=${QW_LOAD1_MAX:-$(sysctl -n hw.logicalcpu)}
gate() {  # cargo idle load1 を 1 回ずつ読む（pkill は打たない・数えるだけ）
  cargo=${QW_CARGO_N:-$(pgrep -x cargo -x rustc 2>/dev/null | wc -l | tr -d ' ')}
  idle=${QW_IDLE_S:-$(python3 -I "$(dirname "$0")/owner_idle.py" --steps "$HOME"/shibubu-yc-runs/*/steps.jsonl 2>/dev/null | awk '{printf "%d", $1}')}  # owner_idle.py = mouse と、自分の type_text を除いた key（W15-b42）
  idle=${idle:-0}
  load1=${QW_LOAD1:-$(sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}')}
  nogo=""  # 空なら go
  if [ "$cargo" -gt "$MAXC" ]; then nogo="cargo:${cargo}"
  elif [ "$idle" -lt "$MIN" ]; then nogo="idle:${idle}"
  elif [ -n "${QW_OPENED_AT:-}" ] && [ "$idle" -lt $(( $(date +%s) - QW_OPENED_AT )) ]; then nogo="owner_since_open:${idle}"  # 窓を開けてからの経過より短い = 窓の途中で人の入力が在った
  elif awk -v a="$load1" -v b="$MAXL" 'BEGIN{exit !(a+0 > b+0)}'; then nogo="load1:${load1}"; fi
}
ROWS=()
while IFS=$'\t' read -r name cwd cmd || [ -n "$name" ]; do
  case $name in ''|'#'*) continue;; esac
  [ -n "$cwd" ] || { echo "quiet-window: 表が壊れている（cwd が空: ${name}）" >&2; exit 2; }
  case $cmd in '#'*) cmd="";; esac
  ROWS+=("$name"$'\t'"${cwd//\$HOME/$HOME}"$'\t'"${cmd//\$HOME/$HOME}")
done < "$PLAN"
[ ${#ROWS[@]} -gt 0 ] || { echo "quiet-window: 表に行が無い" >&2; exit 2; }
if [ $CHK = 1 ]; then
  bad=0; echo "| name | cwd | app | driver | dry-run | tools |"; echo "|---|---|---|---|---|---|"
  for r in "${ROWS[@]}"; do
    IFS=$'\t' read -r n c m <<< "$r"
    if [ -z "$m" ]; then echo "| ${n} | - | - | - | skip (command 空) | - |"; continue; fi
    cw=ok; [ -d "$c" ] || cw=ng
    ap=-; for a in $(grep -o '[^ "]*\.app' <<< "$m"); do [ "$ap" = ng ] || { ap=ok; [ -d "$a" ] || ap=ng; }; done
    dr=ng; dy=-; drv=$(tr ' ' '\n' <<< "$m" | grep -m1 -E '\.(py|sh)$' || true)
    if [ -n "$drv" ] && [ "$cw" = ok ] && [ -f "$c/$drv" ]; then
      dr=ok; interp=bash; case $drv in *.py) interp="python3 -I";; esac
      if grep -q -- '--dry-run' "$c/$drv"; then  # 第 1 引数が --dry-run の driver（gfinal-bg.sh）は app 無し・driver の次が数字（rehearse.sh <n>）は <n> --dry-run・他は <app> --dry-run
        nx=$(tr ' ' '\n' <<< "$m" | grep -A1 -x -F -- "$drv" | sed -n 2p)  # driver の次の語が数字なら <n> --dry-run（rehearse.sh 1 <app>）
        if [[ "$nx" =~ ^([0-9]+|scene2)$ ]]; then args="$nx --dry-run"
        elif grep -q '= "--dry-run"' "$c/$drv"; then args="--dry-run"; else args="$(grep -o '[^ "]*\.app' <<< "$m" | head -1) --dry-run"; fi
        (cd "$c" && $interp "$drv" $args) >/dev/null 2>&1; rc=$?; [ $rc -eq 0 ] && dy=ok || dy="ng($rc)"
      fi
    fi
    tl=-; if [ "$cw" = ok ]; then  # run.sh の start は bin/axprobe 無しだと始まらない（W15-b39）・build.sh が作る
      miss=""; for t in axprobe front-guard windows wincap; do [ -x "$c/scripts/qa/bg/bin/$t" ] || miss="${miss:+$miss,}$t"; done
      tl=ok; [ -z "$miss" ] || tl="ng($miss)"
    fi
    echo "| ${n} | ${cw} | ${ap} | ${dr} | ${dy} | ${tl} |"
    case "$cw$ap$dr$dy$tl" in *ng*) bad=1;; esac
  done
  echo "quiet-window --check: $([ $bad = 0 ] && echo ok || echo ng)"; exit $bad
fi
if [ $DRY = 1 ]; then
  gate; echo "quiet-window: rows=${#ROWS[@]} cargo=${cargo} idle=${idle}s load1=${load1} -> $([ -n "$nogo" ] && echo "no-go($nogo)" || echo go)"
  for r in "${ROWS[@]}"; do IFS=$'\t' read -r n c m <<< "$r"; echo "  ${n}  cwd=${c}  ${m:-(command 空: skip)}"; done
  exit 0
fi
[ -n "$OUT" ] || OUT=$(mktemp -d "$HOME/shibubu-yc-runs/qw.XXXX"); mkdir -p "$OUT"
MD="$OUT/summary.md"
printf '| name | t0 | t1 | 秒 | load1 | cargo | idle | exit | 状態 | run の dir |\n|---|---|---|---|---|---|---|---|---|---|\n' > "$MD"
rc_all=0; stop=""
export QW_OPENED_AT=$(date +%s)  # run.sh の start の門が同じ基準を読む（owner_active）
for r in "${ROWS[@]}"; do
  IFS=$'\t' read -r n c m <<< "$r"
  if [ -z "$m" ]; then echo "| ${n} | - | - | - | - | - | - | - | skip (command 空) | - |" >> "$MD"; continue; fi
  gate; now=$(date +%H:%M:%S)
  if [ -z "$stop" ]; then
    stop=$nogo
  fi
  if [ -n "$stop" ]; then
    echo "| ${n} | $now | - | - | ${load1} | ${cargo} | ${idle} | - | unmeasured (${stop}) | - |" >> "$MD"; rc_all=6; continue
  fi
  s0=$(date +%s)
  (cd "$c" && bash -c "$m") > "$OUT/${n}.log" 2>&1; rc=$?
  s1=$(date +%s); dir=$(grep -o 'out=[^ ]*' "$OUT/${n}.log" | tail -1 | cut -d= -f2-)
  st=ok  # driver の rc 6 = owner が戻った（W15-b3）= 未測。他の非 0 だけ fail
  if [ $rc -eq 6 ]; then st="unmeasured (owner)"; rc_all=6; elif [ $rc -eq 7 ]; then st="unmeasured ($(grep -o 'driver down: .*' "$OUT/${n}.log" | tail -1 | cut -c14-140 | tr '|' '¦'))"; rc_all=6; elif [ $rc -ne 0 ]; then st=fail; [ $rc_all -eq 6 ] || rc_all=1; fi
  echo "| ${n} | $now | $(date +%H:%M:%S) | $((s1-s0)) | ${load1} | ${cargo} | ${idle} | $rc | $st | ${dir:--} |" >> "$MD"
done
cat "$MD"; echo "quiet-window: out=$OUT"
exit $rc_all
